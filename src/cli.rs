use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use http::{HeaderName, HeaderValue};

/// h2load-compatible HTTP benchmark client supporting HTTP/1.1, HTTP/2, and HTTP/3.
#[derive(Parser, Debug, Clone)]
#[command(name = "loadgen-rs", version, about)]
pub struct Cli {
    /// Target URL to benchmark
    pub url: String,

    /// Number of total requests (0 = unlimited when --duration is set)
    #[arg(short = 'n', default_value = "1")]
    pub requests: u64,

    /// Duration of the benchmark run (e.g. "10s", "1m", "500ms").
    /// When set, -n is ignored.
    #[arg(short = 'D', long = "duration", value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// Warm-up duration before measurements start.
    #[arg(long = "warm-up-time", default_value = "0s", value_parser = parse_duration)]
    pub warm_up_time: Duration,

    /// Ramp-up duration for gradually activating clients/lanes before measurements.
    #[arg(long = "ramp-up-time", default_value = "0s", value_parser = parse_duration)]
    pub ramp_up_time: Duration,

    /// Number of concurrent clients/connections
    #[arg(short = 'c', default_value = "1")]
    pub clients: usize,

    /// Number of worker threads (maps to tokio worker_threads)
    #[arg(short = 't', default_value = "1")]
    pub threads: usize,

    /// Max concurrent in-flight requests per connection.
    /// H2/H3: max concurrent streams. H1: max in-flight requests.
    #[arg(short = 'm', default_value = "1")]
    pub max_streams: u32,

    /// Force HTTP/1.1 (overrides --alpn-list)
    #[arg(long = "h1")]
    pub h1: bool,

    /// ALPN protocol selection (h2 or h3)
    #[arg(long = "alpn-list", visible_alias = "alpn")]
    pub alpn_list: Option<AlpnProtocol>,

    /// Force IPv4 for DNS resolution/connect
    #[arg(short = '4', long = "v4", conflicts_with = "v6")]
    pub v4: bool,

    /// Force IPv6 for DNS resolution/connect
    #[arg(short = '6', long = "v6", conflicts_with = "v4")]
    pub v6: bool,

    /// Connection timeout
    #[arg(long = "connect-timeout", default_value = "10s", value_parser = parse_duration)]
    pub connect_timeout: Duration,

    /// Per-request timeout
    #[arg(long = "request-timeout", default_value = "30s", value_parser = parse_duration)]
    pub request_timeout: Duration,

    /// Enable TCP_QUICKACK on Linux for H1/H2 sockets (best-effort)
    #[arg(long = "tcp-quickack")]
    pub tcp_quickack: bool,

    /// HTTP method
    #[arg(long = "method", default_value = "GET")]
    pub method: String,

    /// Additional headers (can be specified multiple times)
    #[arg(long = "header", short = 'H')]
    pub headers: Vec<String>,

    /// Request body data
    #[arg(long = "data", short = 'd')]
    pub data: Option<String>,

    /// Request body from file
    #[arg(long = "data-file")]
    pub data_file: Option<PathBuf>,

    /// Target requests per second per client (h2load-compatible).
    /// Aggregate target ~= (--rps * -c).
    #[arg(long = "rps")]
    pub rps: Option<f64>,

    /// Disable TLS certificate verification
    #[arg(long = "insecure", short = 'k')]
    pub insecure: bool,

    /// TLS cipher suites (rustls format, comma-separated)
    #[arg(long = "tls-ciphers")]
    pub tls_ciphers: Option<String>,

    /// Additional trusted CA certificate (PEM/DER file or directory with PEM/CRT/CER files)
    #[arg(long = "tls-ca")]
    pub tls_ca: Option<PathBuf>,

    /// Output file path (default: stdout)
    #[arg(long = "output", short = 'o')]
    pub output: Option<PathBuf>,

    /// Output format
    #[arg(long = "format", default_value = "jsonl")]
    pub format: OutputFormat,

    /// Disable human-readable summary output (emit only JSONL/CSV machine output)
    #[arg(long = "no-human")]
    pub no_human: bool,

    /// Tail-friendly mode (aims for lower p99, may reduce peak throughput)
    #[arg(long = "tail-friendly")]
    pub tail_friendly: bool,

    /// Record latency/TTFB metrics only for every Nth successful request (1 = all)
    #[arg(long = "metrics-sample", default_value = "1", value_parser = parse_metrics_sample)]
    pub metrics_sample: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Jsonl,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    H1,
    H2,
    H3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AlpnProtocol {
    H2,
    H3,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::H1 => write!(f, "h1"),
            Protocol::H2 => write!(f, "h2"),
            Protocol::H3 => write!(f, "h3"),
        }
    }
}

impl Cli {
    /// Determine the protocol from CLI flags.
    /// --h1 takes priority over --alpn-list.
    pub fn protocol(&self) -> Protocol {
        if self.h1 {
            return Protocol::H1;
        }
        match self.alpn_list {
            Some(AlpnProtocol::H2) => Protocol::H2,
            Some(AlpnProtocol::H3) => Protocol::H3,
            _ => Protocol::H1, // default to HTTP/1.1
        }
    }

    /// Returns true if the run is duration-based (--duration set).
    pub fn is_duration_mode(&self) -> bool {
        self.duration.is_some()
    }

    /// Load request body from --data or --data-file.
    pub fn request_body(&self) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(ref data) = self.data {
            return Ok(Some(data.as_bytes().to_vec()));
        }
        if let Some(ref path) = self.data_file {
            let content = std::fs::read(path)?;
            return Ok(Some(content));
        }
        Ok(None)
    }

    /// Parse headers into typed (name, value) pairs once during CLI parsing.
    pub fn parsed_headers(&self) -> anyhow::Result<Vec<(HeaderName, HeaderValue)>> {
        let mut result = Vec::new();
        for h in &self.headers {
            let (name, value) = h.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("Invalid header format: {h}. Expected 'Name: Value'")
            })?;
            let name = HeaderName::try_from(name.trim())
                .map_err(|e| anyhow::anyhow!("Invalid header name '{}': {e}", name.trim()))?;
            let value = HeaderValue::from_str(value.trim()).map_err(|e| {
                anyhow::anyhow!("Invalid header value for '{}': {e}", name.as_str())
            })?;
            result.push((name, value));
        }
        Ok(result)
    }
}

/// Parse a duration string like "10s", "1m", "500ms", or bare number (seconds).
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        let val: u64 = ms.parse().map_err(|e| format!("Invalid duration: {e}"))?;
        Ok(Duration::from_millis(val))
    } else if let Some(m) = s.strip_suffix('m') {
        // Check it's not "ms" (already handled)
        let val: f64 = m.parse().map_err(|e| format!("Invalid duration: {e}"))?;
        Ok(Duration::from_secs_f64(val * 60.0))
    } else if let Some(secs) = s.strip_suffix('s') {
        let val: f64 = secs.parse().map_err(|e| format!("Invalid duration: {e}"))?;
        Ok(Duration::from_secs_f64(val))
    } else {
        // Bare number = seconds
        let val: f64 = s.parse().map_err(|e| format!("Invalid duration: {e}"))?;
        Ok(Duration::from_secs_f64(val))
    }
}

fn parse_metrics_sample(s: &str) -> Result<u32, String> {
    let val: u32 = s
        .trim()
        .parse()
        .map_err(|e| format!("Invalid metrics sample value: {e}"))?;
    if val == 0 {
        return Err("metrics sample must be >= 1".to_string());
    }
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
    }

    #[test]
    fn test_protocol_detection() {
        let base = Cli::parse_from(["loadgen-rs", "http://localhost"]);
        assert_eq!(base.protocol(), Protocol::H1);
        assert!(!base.tcp_quickack);

        let quickack = Cli::parse_from(["loadgen-rs", "--tcp-quickack", "http://localhost"]);
        assert!(quickack.tcp_quickack);

        let h1 = Cli::parse_from(["loadgen-rs", "--h1", "http://localhost"]);
        assert_eq!(h1.protocol(), Protocol::H1);

        let h2 = Cli::parse_from(["loadgen-rs", "--alpn-list", "h2", "https://localhost"]);
        assert_eq!(h2.protocol(), Protocol::H2);

        let h2_alias = Cli::parse_from(["loadgen-rs", "--alpn", "h2", "https://localhost"]);
        assert_eq!(h2_alias.protocol(), Protocol::H2);

        let h3 = Cli::parse_from(["loadgen-rs", "--alpn-list", "h3", "https://localhost"]);
        assert_eq!(h3.protocol(), Protocol::H3);

        // --h1 overrides --alpn-list
        let override_test = Cli::parse_from([
            "loadgen-rs",
            "--h1",
            "--alpn-list",
            "h2",
            "http://localhost",
        ]);
        assert_eq!(override_test.protocol(), Protocol::H1);

        let v4 = Cli::parse_from(["loadgen-rs", "-4", "http://localhost"]);
        assert!(v4.v4);
        assert!(!v4.v6);

        let v6 = Cli::parse_from(["loadgen-rs", "--v6", "http://localhost"]);
        assert!(!v6.v4);
        assert!(v6.v6);

        let both = Cli::try_parse_from(["loadgen-rs", "-4", "-6", "http://localhost"]);
        assert!(both.is_err());
    }
}
