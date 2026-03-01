use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, Instant};

use bytes::Buf;

use crate::metrics::ErrorClass;
use crate::tls;

use super::{IpVersion, RequestConfig, RequestError, RequestResult, TlsInfo};

#[derive(Debug, Clone)]
pub enum H3Connection {
    Quinn(H3QuinnConnection),
}

impl H3Connection {
    pub fn tls_info(&self) -> &TlsInfo {
        match self {
            H3Connection::Quinn(c) => c.tls_info(),
        }
    }

    pub fn remote_addr(&self) -> SocketAddr {
        match self {
            H3Connection::Quinn(c) => c.remote_addr(),
        }
    }

    pub fn prepare_request_template(&mut self, config: &RequestConfig) {
        match self {
            H3Connection::Quinn(c) => c.prepare_request_template(config),
        }
    }

    pub fn clone_stream_handle(&self) -> Option<Self> {
        match self {
            H3Connection::Quinn(c) => Some(H3Connection::Quinn(c.clone())),
        }
    }

    pub async fn send_request(
        &mut self,
        config: &RequestConfig,
    ) -> Result<RequestResult, RequestError> {
        match self {
            H3Connection::Quinn(c) => c.send_request(config).await,
        }
    }
}

/// Factory for creating HTTP/3 (quinn) connections.
pub enum H3Factory {
    Quinn(H3QuinnFactory),
}

impl H3Factory {
    pub fn from_url(
        url: &http::Uri,
        connect_timeout: Duration,
        insecure: bool,
        tls_ciphers: Option<&str>,
        tls_ca: Option<&Path>,
        ip_version: Option<IpVersion>,
    ) -> Result<Self, RequestError> {
        let quinn_config = tls::build_quinn_client_config(insecure, tls_ciphers, tls_ca)
            .map_err(|e| RequestError::tls(format!("QUIC TLS config failed: {e}")))?;
        let factory = H3QuinnFactory::from_url(url, quinn_config, connect_timeout, ip_version)?;
        Ok(Self::Quinn(factory))
    }

    pub fn supports_multiplexed_lanes(&self) -> bool {
        match self {
            H3Factory::Quinn(_) => true,
        }
    }

    pub async fn connect(&self) -> Result<H3Connection, RequestError> {
        match self {
            H3Factory::Quinn(f) => f.connect().await.map(H3Connection::Quinn),
        }
    }
}

/// HTTP/3 connection using quinn + h3.
#[derive(Clone)]
pub struct H3QuinnConnection {
    send_request: h3::client::SendRequest<h3_quinn::OpenStreams, bytes::Bytes>,
    remote_addr: SocketAddr,
    request_template: Option<http::Request<()>>,
    request_body: Option<bytes::Bytes>,
    bytes_out_estimate: Option<u64>,
    tls_info: TlsInfo,
}

impl std::fmt::Debug for H3QuinnConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H3QuinnConnection")
            .field("remote_addr", &self.remote_addr)
            .field(
                "request_template_prepared",
                &self.request_template.is_some(),
            )
            .field("request_body_prepared", &self.request_body.is_some())
            .field("bytes_out_estimate", &self.bytes_out_estimate)
            .field("tls_info", &self.tls_info)
            .finish()
    }
}

impl H3QuinnConnection {
    pub fn tls_info(&self) -> &TlsInfo {
        &self.tls_info
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub fn prepare_request_template(&mut self, config: &RequestConfig) {
        if self.request_template.is_none() {
            let mut request = http::Request::new(());
            *request.method_mut() = config.method.clone();
            *request.uri_mut() = config.url.clone();
            request.headers_mut().extend(config.headers.iter().cloned());
            self.request_template = Some(request);
        }
        if self.request_body.is_none()
            && let Some(body) = &config.body
        {
            self.request_body = Some(body.clone());
        }
        if self.bytes_out_estimate.is_none() {
            self.bytes_out_estimate = Some(estimate_h3_request_size(config));
        }
    }

    pub async fn send_request(
        &mut self,
        config: &RequestConfig,
    ) -> Result<RequestResult, RequestError> {
        if self.request_template.is_none()
            || self.bytes_out_estimate.is_none()
            || (config.body.is_some() && self.request_body.is_none())
        {
            self.prepare_request_template(config);
        }

        let start = Instant::now();

        let bytes_out = self
            .bytes_out_estimate
            .expect("h3 bytes_out estimate should be prepared");

        let request = self
            .request_template
            .as_ref()
            .expect("h3 request template should be prepared")
            .clone();
        let request_body = self.request_body.as_ref().cloned();

        let fut = async {
            let mut stream = self
                .send_request
                .send_request(request)
                .await
                .map_err(|e| RequestError::http(format!("H3 send failed: {e}")))?;

            // Send body if present
            if let Some(body) = request_body {
                stream
                    .send_data(body)
                    .await
                    .map_err(|e| RequestError::http(format!("H3 body send failed: {e}")))?;
            }

            stream
                .finish()
                .await
                .map_err(|e| RequestError::http(format!("H3 finish failed: {e}")))?;

            let response = stream
                .recv_response()
                .await
                .map_err(|e| RequestError::http(format!("H3 recv response failed: {e}")))?;

            let header_ttfb = start.elapsed();
            let status = response.status().as_u16();

            // Read response body
            let mut bytes_in = 0u64;
            let mut ttfb_us = header_ttfb.as_micros() as u64;
            let mut saw_body = false;
            while let Some(chunk) = stream
                .recv_data()
                .await
                .map_err(|e| RequestError::http(format!("H3 recv data failed: {e}")))?
            {
                if !saw_body {
                    ttfb_us = start.elapsed().as_micros() as u64;
                    saw_body = true;
                }
                bytes_in += chunk.remaining() as u64;
            }

            let latency = start.elapsed();
            Ok::<RequestResult, RequestError>(RequestResult {
                status,
                latency_us: latency.as_micros() as u64,
                ttfb_us,
                bytes_in,
                bytes_out,
            })
        };

        tokio::time::timeout(config.request_timeout, fut)
            .await
            .map_err(|_| RequestError::timeout("Request timed out"))?
    }
}

pub struct H3QuinnFactory {
    quinn_config: quinn::ClientConfig,
    server_addrs: Vec<SocketAddr>,
    server_name: String,
    connect_timeout: Duration,
}

impl H3QuinnFactory {
    pub fn new(
        quinn_config: quinn::ClientConfig,
        server_addrs: Vec<SocketAddr>,
        server_name: String,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            quinn_config,
            server_addrs,
            server_name,
            connect_timeout,
        }
    }

    pub fn from_url(
        url: &http::Uri,
        quinn_config: quinn::ClientConfig,
        connect_timeout: Duration,
        ip_version: Option<IpVersion>,
    ) -> Result<Self, RequestError> {
        let (server_addrs, server_name) = parse_server_targets(url, ip_version)?;
        Ok(Self::new(
            quinn_config,
            server_addrs,
            server_name,
            connect_timeout,
        ))
    }

    pub async fn connect(&self) -> Result<H3QuinnConnection, RequestError> {
        let mut last_connect_error: Option<RequestError> = None;

        for &server_addr in &self.server_addrs {
            match self.connect_single(server_addr).await {
                Ok(conn) => return Ok(conn),
                Err(e) => {
                    if matches!(e.class, ErrorClass::Connect | ErrorClass::Timeout) {
                        last_connect_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_connect_error
            .unwrap_or_else(|| RequestError::connect("No resolved addresses to connect")))
    }

    async fn connect_single(
        &self,
        server_addr: SocketAddr,
    ) -> Result<H3QuinnConnection, RequestError> {
        // Create a UDP socket with enlarged send/receive buffers to reduce
        // kernel drops under high-throughput QUIC workloads.
        let socket = socket2::Socket::new(
            socket2::Domain::for_address(server_addr),
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| RequestError::connect(format!("UDP socket creation failed: {e}")))?;
        let local_bind_addr = match server_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse::<SocketAddr>().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse::<SocketAddr>().unwrap(),
        };
        socket
            .bind(&local_bind_addr.into())
            .map_err(|e| RequestError::connect(format!("UDP socket bind failed: {e}")))?;
        // Best-effort: kernel may cap to net.core.wmem_max / rmem_max.
        let _ = socket.set_send_buffer_size(4 * 1024 * 1024);
        let _ = socket.set_recv_buffer_size(4 * 1024 * 1024);

        let runtime = quinn::default_runtime()
            .ok_or_else(|| RequestError::connect("No async runtime found for quinn"))?;
        let mut endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            None,
            socket.into(),
            runtime,
        )
        .map_err(|e| RequestError::connect(format!("Failed to create QUIC endpoint: {e}")))?;
        endpoint.set_default_client_config(self.quinn_config.clone());

        let connecting = endpoint
            .connect(server_addr, &self.server_name)
            .map_err(|e| RequestError::connect(format!("QUIC connect failed: {e}")))?;

        let connection = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| RequestError::timeout("QUIC connect timed out"))?
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("tls") || msg.contains("certificate") {
                    RequestError::tls(msg)
                } else {
                    RequestError::connect(msg)
                }
            })?;

        let mut builder = h3::client::builder();
        // Disable GREASE frame emission for benchmark comparability / lower control overhead.
        builder.send_grease(false);
        let (mut driver, send_request) = builder
            .build(h3_quinn::Connection::new(connection))
            .await
            .map_err(|e| RequestError::http(format!("H3 handshake failed: {e}")))?;

        tokio::spawn(async move {
            let err = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
            tracing::debug!("H3 connection closed: {err}");
        });

        Ok(H3QuinnConnection {
            send_request,
            remote_addr: server_addr,
            request_template: None,
            request_body: None,
            bytes_out_estimate: None,
            // QUIC always uses TLS 1.3; quinn does not currently expose negotiated TLS cipher here.
            tls_info: TlsInfo {
                protocol: Some("TLSv1.3".to_string()),
                cipher: None,
            },
        })
    }
}

fn parse_server_targets(
    url: &http::Uri,
    ip_version: Option<IpVersion>,
) -> Result<(Vec<SocketAddr>, String), RequestError> {
    let host = url
        .host()
        .ok_or_else(|| RequestError::connect("URL has no host"))?;
    let port = url.port_u16().unwrap_or(443);

    let mut addrs = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| RequestError::connect(format!("DNS resolution failed: {e}")))?
        .collect::<Vec<_>>();
    if let Some(version) = ip_version {
        addrs.retain(|addr| {
            matches!(
                (version, addr),
                (IpVersion::V4, SocketAddr::V4(_)) | (IpVersion::V6, SocketAddr::V6(_))
            )
        });
    }
    if addrs.is_empty() {
        let detail = match ip_version {
            Some(IpVersion::V4) => "DNS resolution returned no IPv4 addresses",
            Some(IpVersion::V6) => "DNS resolution returned no IPv6 addresses",
            None => "DNS resolution returned no addresses",
        };
        return Err(RequestError::connect(detail));
    }

    Ok((addrs, host.to_string()))
}

fn estimate_h3_request_size(config: &RequestConfig) -> u64 {
    // QUIC frame overhead + QPACK compressed headers
    let mut size = 20u64; // approximate QUIC overhead
    size += config.method.as_str().len() as u64;
    size += config
        .url
        .path_and_query()
        .map_or(1, |pq| pq.as_str().len()) as u64;
    for (name, value) in &config.headers {
        size += (name.as_str().len() + value.as_bytes().len() + 2) as u64;
    }
    if let Some(ref body) = config.body {
        size += body.len() as u64;
    }
    size
}
