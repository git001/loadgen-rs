use std::sync::Arc;
use std::{io::Cursor, path::Path};

use anyhow::{Context, Result};
use rustls::crypto::ring as ring_provider;
use rustls::pki_types::ServerName;

use crate::cli::Protocol;

/// Build a rustls ClientConfig for HTTP/1.1 and HTTP/2.
pub fn build_rustls_config(
    protocol: Protocol,
    insecure: bool,
    tls_ciphers: Option<&str>,
    tls_ca: Option<&Path>,
) -> Result<Arc<rustls::ClientConfig>> {
    let cipher_suites = if let Some(ciphers) = tls_ciphers {
        parse_cipher_suites(ciphers, protocol)?
    } else {
        ring_provider::default_provider().cipher_suites.clone()
    };

    let provider = Arc::new(rustls::crypto::CryptoProvider {
        cipher_suites,
        ..ring_provider::default_provider()
    });

    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("Failed to set TLS protocol versions")?;

    let mut config = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
            .with_no_client_auth()
    } else {
        let mut root_store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(ca_path) = tls_ca {
            let added = load_custom_ca_into_store(&mut root_store, ca_path)?;
            tracing::info!(
                "Loaded {} custom CA certificate(s) from '{}'",
                added,
                ca_path.display()
            );
        }
        builder
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    // Explicit ALPN configuration per protocol.
    match protocol {
        Protocol::H1 => {}
        Protocol::H2 => config.alpn_protocols = vec![b"h2".to_vec()],
        Protocol::H3 => config.alpn_protocols = vec![b"h3".to_vec()],
    }

    Ok(Arc::new(config))
}

/// Build a quinn ClientConfig for HTTP/3 (QUIC).
pub fn build_quinn_client_config(
    insecure: bool,
    tls_ciphers: Option<&str>,
    tls_ca: Option<&Path>,
) -> Result<quinn::ClientConfig> {
    let tls_config = build_rustls_config(Protocol::H3, insecure, tls_ciphers, tls_ca)?;
    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
        .context("Invalid QUIC TLS config")?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_config));

    // QUIC transport defaults tuned for high-throughput benchmarking.
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(quinn::VarInt::from_u32(1024));
    transport.max_concurrent_uni_streams(quinn::VarInt::from_u32(256));
    // Match h2load's 1 GB stream/connection windows (2^30 - 1).
    transport.stream_receive_window(quinn::VarInt::from_u32(1 << 30));
    transport.receive_window(quinn::VarInt::from_u32(1 << 30));
    transport.send_window(1 << 30);
    transport.send_fairness(false);
    transport.initial_mtu(1472);
    transport.min_mtu(1200);
    transport.enable_segmentation_offload(true);
    // Quinn defaults to 333 ms initial RTT which cripples congestion control
    // ramp-up on low-latency links.  Use 1 ms as a reasonable starting point;
    // the CC will converge to the real RTT after the first few packets.
    transport.initial_rtt(std::time::Duration::from_millis(1));
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
    ));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
    client_config.transport_config(Arc::new(transport));

    Ok(client_config)
}

fn load_custom_ca_into_store(root_store: &mut rustls::RootCertStore, path: &Path) -> Result<usize> {
    if path.is_file() {
        return load_ca_file_into_store(root_store, path);
    }
    if !path.is_dir() {
        anyhow::bail!(
            "Custom CA path '{}' is neither a file nor a directory",
            path.display()
        );
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("Failed to read custom CA directory '{}'", path.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "Failed to read an entry from custom CA directory '{}'",
                path.display()
            )
        })?;
        let file_type = entry.file_type().with_context(|| {
            format!("Failed to read file type for '{}'", entry.path().display())
        })?;
        if !file_type.is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if matches!(ext.as_deref(), Some("pem" | "crt" | "cer")) {
            files.push(entry.path());
        }
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!(
            "Custom CA directory '{}' does not contain PEM/CRT/CER files",
            path.display()
        );
    }

    let mut added_total = 0usize;
    for file in files {
        added_total += load_ca_file_into_store(root_store, &file)?;
    }
    Ok(added_total)
}

fn load_ca_file_into_store(root_store: &mut rustls::RootCertStore, file: &Path) -> Result<usize> {
    let bytes = std::fs::read(file)
        .with_context(|| format!("Failed to read custom CA file '{}'", file.display()))?;

    let mut reader = Cursor::new(bytes.as_slice());
    let mut added = 0usize;
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.with_context(|| {
            format!(
                "Failed to parse certificate entry in custom CA file '{}'",
                file.display()
            )
        })?;
        root_store.add(cert).with_context(|| {
            format!(
                "Failed to add certificate from custom CA file '{}'",
                file.display()
            )
        })?;
        added += 1;
    }

    if added == 0 {
        // Fallback for DER-encoded single certificate files.
        root_store
            .add(rustls::pki_types::CertificateDer::from(bytes))
            .with_context(|| {
                format!(
                    "Custom CA file '{}' is neither valid PEM bundle nor DER cert",
                    file.display()
                )
            })?;
        added = 1;
    }

    Ok(added)
}

/// Parse comma-separated cipher suite names into rustls SupportedCipherSuite.
fn parse_cipher_suites(
    ciphers: &str,
    protocol: Protocol,
) -> Result<Vec<rustls::SupportedCipherSuite>> {
    let all_suites = ring_provider::default_provider().cipher_suites.clone();
    let mut result = Vec::new();

    for name in ciphers.split(',').map(|s| s.trim()) {
        let suite = all_suites
            .iter()
            .find(|s| format!("{:?}", s.suite()).contains(name))
            .ok_or_else(|| anyhow::anyhow!("Unknown cipher suite: {name}"))?;

        // H3 (QUIC) requires TLS 1.3 cipher suites only
        if protocol == Protocol::H3 {
            match suite.suite() {
                rustls::CipherSuite::TLS13_AES_128_GCM_SHA256
                | rustls::CipherSuite::TLS13_AES_256_GCM_SHA384
                | rustls::CipherSuite::TLS13_CHACHA20_POLY1305_SHA256 => {}
                other => {
                    anyhow::bail!(
                        "Cipher suite {other:?} is not valid for HTTP/3 (requires TLS 1.3)"
                    );
                }
            }
        }

        result.push(*suite);
    }

    if result.is_empty() {
        anyhow::bail!("No valid cipher suites specified");
    }

    Ok(result)
}

/// A certificate verifier that accepts everything (for --insecure).
#[derive(Debug)]
struct InsecureVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}
