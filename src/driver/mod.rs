pub mod h1_raw;
pub mod h2;
pub mod h3;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use rustls::ClientConnection;
use tokio::net::TcpStream;

use crate::metrics::ErrorClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpVersion {
    V4,
    V6,
}

/// Result of a single HTTP request.
#[derive(Debug)]
pub struct RequestResult {
    /// HTTP status code (0 if connection/TLS error).
    pub status: u16,
    /// End-to-end latency in microseconds.
    pub latency_us: u64,
    /// Time to first byte in microseconds.
    pub ttfb_us: u64,
    /// Response body bytes received.
    pub bytes_in: u64,
    /// Request bytes sent (approx).
    pub bytes_out: u64,
}

/// Error from a benchmark request with classification.
#[derive(Debug)]
pub struct RequestError {
    pub class: ErrorClass,
    pub message: String,
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.class, self.message)
    }
}

impl std::error::Error for RequestError {}

impl RequestError {
    pub fn connect(msg: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Connect,
            message: msg.into(),
        }
    }

    pub fn tls(msg: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Tls,
            message: msg.into(),
        }
    }

    pub fn timeout(msg: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Timeout,
            message: msg.into(),
        }
    }

    pub fn http(msg: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Http,
            message: msg.into(),
        }
    }
}

/// Configuration passed to each driver/connection.
#[derive(Debug, Clone)]
pub struct RequestConfig {
    pub url: http::Uri,
    pub method: http::Method,
    pub headers: Vec<(http::HeaderName, http::HeaderValue)>,
    pub body: Option<Bytes>,
    pub request_timeout: Duration,
    pub tail_friendly: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TlsInfo {
    pub protocol: Option<String>,
    pub cipher: Option<String>,
}

impl TlsInfo {
    pub fn is_empty(&self) -> bool {
        self.protocol.is_none() && self.cipher.is_none()
    }
}

pub fn tls_info_from_rustls_conn(conn: &ClientConnection) -> TlsInfo {
    let protocol = conn.protocol_version().map(format_rustls_protocol);
    let cipher = conn
        .negotiated_cipher_suite()
        .map(|suite| format_rustls_cipher(suite.suite()));
    TlsInfo { protocol, cipher }
}

pub fn maybe_enable_tcp_quickack(stream: &TcpStream, enabled: bool) {
    if !enabled {
        return;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;

        let fd = stream.as_raw_fd();
        let optval: libc::c_int = 1;
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_TCP,
                libc::TCP_QUICKACK,
                (&optval as *const libc::c_int).cast::<libc::c_void>(),
                std::mem::size_of_val(&optval) as libc::socklen_t,
            )
        };
        if rc != 0 {
            tracing::debug!(
                "Failed to set TCP_QUICKACK on socket: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = stream;
    }
}

fn format_rustls_protocol(version: rustls::ProtocolVersion) -> String {
    match version {
        rustls::ProtocolVersion::TLSv1_3 => "TLSv1.3".to_string(),
        rustls::ProtocolVersion::TLSv1_2 => "TLSv1.2".to_string(),
        _ => format!("{version:?}"),
    }
}

fn format_rustls_cipher(cipher: rustls::CipherSuite) -> String {
    let raw = format!("{cipher:?}");
    if let Some(suffix) = raw.strip_prefix("TLS13_") {
        format!("TLS_{suffix}")
    } else {
        raw
    }
}

/// Enum-based connection dispatch (avoids async dyn trait boxing).
#[derive(Debug)]
pub enum Connection {
    H1Raw(h1_raw::H1RawConnection),
    H2(h2::H2Connection),
    H3(h3::H3Connection),
}

impl Connection {
    pub fn prepare_request_template(&mut self, config: &RequestConfig) {
        match self {
            Connection::H1Raw(c) => c.prepare_request_template(config),
            Connection::H2(c) => c.prepare_request_template(config),
            Connection::H3(c) => c.prepare_request_template(config),
        }
    }

    pub fn clone_stream_handle(&self) -> Option<Self> {
        match self {
            Connection::H2(c) => Some(Connection::H2(c.clone())),
            Connection::H3(c) => c.clone_stream_handle().map(Connection::H3),
            _ => None,
        }
    }

    pub fn tls_info(&self) -> Option<&TlsInfo> {
        let info = match self {
            Connection::H1Raw(c) => c.tls_info(),
            Connection::H2(c) => c.tls_info(),
            Connection::H3(c) => c.tls_info(),
        };
        if info.is_empty() { None } else { Some(info) }
    }

    pub fn remote_addr(&self) -> SocketAddr {
        match self {
            Connection::H1Raw(c) => c.remote_addr(),
            Connection::H2(c) => c.remote_addr(),
            Connection::H3(c) => c.remote_addr(),
        }
    }

    pub async fn send_request(
        &mut self,
        config: &RequestConfig,
    ) -> Result<RequestResult, RequestError> {
        match self {
            Connection::H1Raw(c) => c.send_request(config).await,
            Connection::H2(c) => c.send_request(config).await,
            Connection::H3(c) => c.send_request(config).await,
        }
    }
}

/// Factory that creates connections for a given protocol.
pub enum ConnectionFactory {
    H1Raw(h1_raw::H1RawFactory),
    H2(h2::H2Factory),
    H3(h3::H3Factory),
}

impl ConnectionFactory {
    pub fn supports_multiplexed_lanes(&self) -> bool {
        match self {
            ConnectionFactory::H1Raw(_) => false,
            ConnectionFactory::H2(_) => true,
            ConnectionFactory::H3(f) => f.supports_multiplexed_lanes(),
        }
    }

    pub async fn create_connection(&self) -> Result<Connection, RequestError> {
        match self {
            ConnectionFactory::H1Raw(f) => f.connect().await.map(Connection::H1Raw),
            ConnectionFactory::H2(f) => f.connect().await.map(Connection::H2),
            ConnectionFactory::H3(f) => f.connect().await.map(Connection::H3),
        }
    }
}
