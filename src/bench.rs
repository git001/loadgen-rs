use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::cli::Protocol;
use crate::driver::{ConnectionFactory, IpVersion, RequestConfig};
use crate::metrics::RunReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BenchProtocol {
    #[default]
    H1,
    H2,
    H3,
}

impl From<BenchProtocol> for Protocol {
    fn from(value: BenchProtocol) -> Self {
        match value {
            BenchProtocol::H1 => Protocol::H1,
            BenchProtocol::H2 => Protocol::H2,
            BenchProtocol::H3 => Protocol::H3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchConfig {
    pub url: String,
    #[serde(default)]
    pub protocol: BenchProtocol,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: Vec<BenchHeader>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default = "default_requests")]
    pub requests: u64,
    #[serde(default)]
    pub duration_s: Option<f64>,
    #[serde(default)]
    pub warm_up_time_s: f64,
    #[serde(default)]
    pub ramp_up_time_s: f64,
    #[serde(default = "default_clients")]
    pub clients: usize,
    #[serde(default = "default_threads")]
    pub threads: usize,
    #[serde(default = "default_max_streams")]
    pub max_streams: u32,
    #[serde(default = "default_connect_timeout_s")]
    pub connect_timeout_s: f64,
    #[serde(default = "default_request_timeout_s")]
    pub request_timeout_s: f64,
    #[serde(default)]
    pub rps: Option<f64>,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub tls_ciphers: Option<String>,
    #[serde(default)]
    pub tls_ca: Option<String>,
    #[serde(default)]
    pub tail_friendly: bool,
    #[serde(default = "default_metrics_sample")]
    pub metrics_sample: u32,
    #[serde(default)]
    pub tcp_quickack: bool,
    #[serde(default)]
    pub v4: bool,
    #[serde(default)]
    pub v6: bool,
    #[serde(default)]
    pub export_histograms: bool,
}

fn default_method() -> String {
    "GET".to_string()
}

const fn default_requests() -> u64 {
    1
}

const fn default_clients() -> usize {
    1
}

const fn default_threads() -> usize {
    1
}

const fn default_max_streams() -> u32 {
    1
}

const fn default_connect_timeout_s() -> f64 {
    10.0
}

const fn default_request_timeout_s() -> f64 {
    30.0
}

const fn default_metrics_sample() -> u32 {
    1
}

pub fn run_from_config(config: BenchConfig) -> Result<RunReport> {
    validate_config(&config)?;

    let protocol: Protocol = config.protocol.into();
    let url: http::Uri = config.url.parse().context("Invalid URL")?;

    match url.scheme_str() {
        Some("http") | Some("https") => {}
        Some(s) => anyhow::bail!("Unsupported URL scheme: {s}"),
        None => anyhow::bail!("URL must have a scheme (http:// or https://)"),
    }

    maybe_warn_container_localhost_target(&url);
    maybe_warn_tcp_quickack(protocol, config.tcp_quickack);

    let method: http::Method = config.method.parse().context("Invalid HTTP method")?;
    let headers = parse_headers(&config.headers)?;
    let body = config
        .body
        .as_ref()
        .map(|v| Bytes::copy_from_slice(v.as_bytes()));

    let request_config = Arc::new(RequestConfig {
        url: url.clone(),
        method,
        headers,
        body,
        request_timeout: duration_from_secs("request_timeout_s", config.request_timeout_s)?,
        tail_friendly: config.tail_friendly,
    });

    let factory = Arc::new(build_factory(&config, protocol, &url)?);

    let duration = match config.duration_s {
        Some(v) => Some(duration_from_secs("duration_s", v)?),
        None => None,
    };
    let warm_up_time = duration_from_secs("warm_up_time_s", config.warm_up_time_s)?;
    let ramp_up_time = duration_from_secs("ramp_up_time_s", config.ramp_up_time_s)?;

    let (total_requests, mode_str) = if duration.is_some() {
        (u64::MAX, "duration")
    } else {
        (config.requests, "count")
    };

    let (worker_metrics, elapsed) = crate::runner::run_benchmark(
        factory,
        request_config,
        config.clients,
        config.max_streams,
        config.threads,
        config.tail_friendly,
        config.metrics_sample,
        total_requests,
        duration,
        warm_up_time,
        ramp_up_time,
        config.rps,
    );

    Ok(crate::metrics::merge_metrics(
        worker_metrics,
        elapsed,
        &protocol.to_string(),
        &config.url,
        (protocol == Protocol::H3).then_some("quinn"),
        config.clients,
        config.threads,
        config.max_streams,
        mode_str,
        config.duration_s.unwrap_or(0.0),
        config.metrics_sample,
        config.requests,
        config
            .rps
            .map(|per_client| per_client * config.clients as f64),
        config.export_histograms,
    ))
}

fn validate_config(config: &BenchConfig) -> Result<()> {
    if config.clients == 0 {
        anyhow::bail!("clients must be >= 1");
    }
    if config.threads == 0 {
        anyhow::bail!("threads must be >= 1");
    }
    if config.max_streams == 0 {
        anyhow::bail!("max_streams must be >= 1");
    }
    if config.metrics_sample == 0 {
        anyhow::bail!("metrics_sample must be >= 1");
    }
    if let Some(rps) = config.rps
        && (!rps.is_finite() || rps <= 0.0)
    {
        anyhow::bail!("rps must be > 0");
    }
    if config.v4 && config.v6 {
        anyhow::bail!("v4 and v6 cannot both be true");
    }
    Ok(())
}

fn parse_headers(headers: &[BenchHeader]) -> Result<Vec<(HeaderName, HeaderValue)>> {
    let mut parsed = Vec::with_capacity(headers.len());
    for h in headers {
        let name = HeaderName::try_from(h.name.trim())
            .map_err(|e| anyhow::anyhow!("Invalid header name '{}': {e}", h.name.trim()))?;
        let value = HeaderValue::from_str(h.value.trim())
            .map_err(|e| anyhow::anyhow!("Invalid header value for '{}': {e}", name.as_str()))?;
        parsed.push((name, value));
    }
    Ok(parsed)
}

fn duration_from_secs(field: &str, value: f64) -> Result<Duration> {
    if !value.is_finite() || value < 0.0 {
        anyhow::bail!("{field} must be a finite number >= 0");
    }
    Ok(Duration::from_secs_f64(value))
}

fn build_factory(
    config: &BenchConfig,
    protocol: Protocol,
    url: &http::Uri,
) -> Result<ConnectionFactory> {
    let tls_ca_path = config.tls_ca.as_deref().map(Path::new);
    let forced_ip_version = if config.v4 {
        Some(IpVersion::V4)
    } else if config.v6 {
        Some(IpVersion::V6)
    } else {
        None
    };

    let connect_timeout = duration_from_secs("connect_timeout_s", config.connect_timeout_s)?;

    match protocol {
        Protocol::H1 => {
            let tls_config = crate::tls::build_rustls_config(
                Protocol::H1,
                config.insecure,
                config.tls_ciphers.as_deref(),
                tls_ca_path,
            )?;
            Ok(ConnectionFactory::H1Raw(
                crate::driver::h1_raw::H1RawFactory::from_url(
                    url,
                    tls_config,
                    connect_timeout,
                    config.tcp_quickack,
                    forced_ip_version,
                )?,
            ))
        }
        Protocol::H2 => {
            let tls_config = crate::tls::build_rustls_config(
                Protocol::H2,
                config.insecure,
                config.tls_ciphers.as_deref(),
                tls_ca_path,
            )?;
            Ok(ConnectionFactory::H2(
                crate::driver::h2::H2Factory::from_url(
                    url,
                    tls_config,
                    connect_timeout,
                    config.tcp_quickack,
                    forced_ip_version,
                )?,
            ))
        }
        Protocol::H3 => {
            let factory = crate::driver::h3::H3Factory::from_url(
                url,
                connect_timeout,
                config.insecure,
                config.tls_ciphers.as_deref(),
                tls_ca_path,
                forced_ip_version,
            )?;
            Ok(ConnectionFactory::H3(factory))
        }
    }
}

fn maybe_warn_container_localhost_target(url: &http::Uri) {
    let host = match url.host() {
        Some(host) => host,
        None => return,
    };

    let is_local_target = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !is_local_target || !running_in_container() || !likely_isolated_container_network() {
        return;
    }

    tracing::warn!(
        "Target host '{}' is loopback inside a container with likely isolated networking. If you intended the host machine, use --network host or a non-loopback host/IP.",
        host
    );
}

fn running_in_container() -> bool {
    std::env::var_os("container").is_some()
        || std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
}

fn likely_isolated_container_network() -> bool {
    let entries = match std::fs::read_dir("/sys/class/net") {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    let mut non_loopback = 0usize;
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str()
            && name != "lo"
        {
            non_loopback += 1;
        }
    }

    non_loopback <= 1
}

fn maybe_warn_tcp_quickack(protocol: Protocol, enabled: bool) {
    if !enabled {
        return;
    }

    if protocol == Protocol::H3 {
        tracing::warn!("tcp_quickack applies only to TCP (H1/H2); ignored for h3");
    }

    #[cfg(not(target_os = "linux"))]
    tracing::warn!("tcp_quickack is currently only supported on Linux");
}
