use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use tokio::net::{TcpSocket, TcpStream};
use tokio_rustls::TlsConnector;

use super::{
    IpVersion, RequestConfig, RequestError, RequestResult, TlsInfo, maybe_enable_tcp_quickack,
    tls_info_from_rustls_conn,
};

trait IoStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T> IoStream for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
type BoxIo = Box<dyn IoStream>;

/// HTTP/2 connection using direct h2 crate (no hyper).
#[derive(Debug, Clone)]
pub struct H2Connection {
    sender: h2::client::SendRequest<bytes::Bytes>,
    remote_addr: SocketAddr,
    request_template: Option<http::Request<()>>,
    request_body: Option<bytes::Bytes>,
    end_of_stream: bool,
    bytes_out_estimate: Option<u64>,
    tls_info: TlsInfo,
}

const H2_STREAM_WINDOW: u32 = 1 << 30;
const H2_CONN_WINDOW: u32 = 1 << 30;
const H2_MAX_FRAME: u32 = 1_048_576;
const H2_RELEASE_BATCH_BYTES: usize = 512 * 1024;

impl H2Connection {
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
            *request.version_mut() = http::Version::HTTP_2;
            request.headers_mut().extend(config.headers.iter().cloned());
            self.request_template = Some(request);
        }
        if self.bytes_out_estimate.is_none() {
            self.bytes_out_estimate = Some(estimate_h2_request_size(config));
        }
        if self.request_body.is_none()
            && let Some(body) = &config.body
        {
            self.request_body = Some(body.clone());
        }
        self.end_of_stream = self.request_body.is_none();
    }

    pub async fn send_request(
        &mut self,
        config: &RequestConfig,
    ) -> Result<RequestResult, RequestError> {
        self.prepare_request_template(config);
        let start = Instant::now();
        let bytes_out = self
            .bytes_out_estimate
            .expect("h2 bytes_out estimate should be prepared");
        let request_template = self
            .request_template
            .as_ref()
            .expect("h2 request template should be prepared");
        let request_body = self.request_body.as_ref();
        let end_of_stream = self.end_of_stream;

        let fut = async {
            let request = request_template.clone();

            let (response_fut, mut send_stream) = self
                .sender
                .send_request(request, end_of_stream)
                .map_err(classify_h2_error)?;

            if let Some(body) = request_body {
                send_stream
                    .send_data(body.clone(), true)
                    .map_err(classify_h2_error)?;
            }

            let response = response_fut.await.map_err(classify_h2_error)?;
            let ttfb = start.elapsed();
            let status = response.status().as_u16();
            let mut recv = response.into_body();
            let mut bytes_in = 0u64;

            // Release flow-control capacity in batches to reduce the overhead
            // of too-frequent WINDOW_UPDATE signaling on high-throughput runs.
            let mut pending_release = 0usize;
            while let Some(chunk) = recv.data().await {
                let chunk = chunk.map_err(classify_h2_error)?;
                let len = chunk.len();
                bytes_in += len as u64;
                pending_release = pending_release.saturating_add(len);
                if pending_release >= H2_RELEASE_BATCH_BYTES {
                    recv.flow_control()
                        .release_capacity(pending_release)
                        .map_err(classify_h2_error)?;
                    pending_release = 0;
                }
            }

            if pending_release > 0 {
                recv.flow_control()
                    .release_capacity(pending_release)
                    .map_err(classify_h2_error)?;
            }

            let latency = start.elapsed();
            Ok::<RequestResult, RequestError>(RequestResult {
                status,
                latency_us: latency.as_micros() as u64,
                ttfb_us: ttfb.as_micros() as u64,
                bytes_in,
                bytes_out,
            })
        };

        tokio::time::timeout(config.request_timeout, fut)
            .await
            .map_err(|_| RequestError::timeout("Request timed out"))?
    }
}

/// Factory for creating HTTP/2 connections.
pub struct H2Factory {
    tls_config: Arc<rustls::ClientConfig>,
    server_addrs: Vec<SocketAddr>,
    server_name: String,
    connect_timeout: Duration,
    tcp_quickack: bool,
    use_tls: bool,
}

impl H2Factory {
    pub fn from_url(
        url: &http::Uri,
        tls_config: Arc<rustls::ClientConfig>,
        connect_timeout: Duration,
        tcp_quickack: bool,
        ip_version: Option<IpVersion>,
    ) -> Result<Self, RequestError> {
        let host = url
            .host()
            .ok_or_else(|| RequestError::connect("URL has no host"))?;
        let scheme = url.scheme_str().unwrap_or("https");
        let port = url
            .port_u16()
            .unwrap_or(if scheme.eq_ignore_ascii_case("https") {
                443
            } else {
                80
            });

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

        Ok(Self {
            tls_config,
            server_addrs: addrs,
            server_name: host.to_string(),
            connect_timeout,
            tcp_quickack,
            use_tls: scheme.eq_ignore_ascii_case("https"),
        })
    }

    pub async fn connect(&self) -> Result<H2Connection, RequestError> {
        let (tcp, remote_addr) =
            connect_tcp_with_fallback(&self.server_addrs, self.connect_timeout).await?;
        let _ = tcp.set_nodelay(true);
        maybe_enable_tcp_quickack(&tcp, self.tcp_quickack);

        let (io, tls_info): (BoxIo, TlsInfo) = if self.use_tls {
            let connector = TlsConnector::from(self.tls_config.clone());
            let server_name = ServerName::try_from(self.server_name.clone())
                .map_err(|e| RequestError::tls(format!("Invalid TLS server name: {e}")))?;
            let tls_stream =
                tokio::time::timeout(self.connect_timeout, connector.connect(server_name, tcp))
                    .await
                    .map_err(|_| RequestError::timeout("TLS handshake timed out"))?
                    .map_err(|e| RequestError::tls(format!("TLS handshake failed: {e}")))?;
            let info = tls_info_from_rustls_conn(tls_stream.get_ref().1);
            (Box::new(tls_stream), info)
        } else {
            (Box::new(tcp), TlsInfo::default())
        };

        let mut builder = h2::client::Builder::new();
        builder
            .initial_window_size(H2_STREAM_WINDOW)
            .initial_connection_window_size(H2_CONN_WINDOW)
            .max_frame_size(H2_MAX_FRAME)
            .initial_max_send_streams(1_000)
            .max_concurrent_streams(1_000);

        let (sender, connection) =
            tokio::time::timeout(self.connect_timeout, builder.handshake(io))
                .await
                .map_err(|_| RequestError::timeout("H2 handshake timed out"))?
                .map_err(classify_h2_error)?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!("H2 connection driver closed: {e}");
            }
        });

        Ok(H2Connection {
            sender,
            remote_addr,
            request_template: None,
            request_body: None,
            end_of_stream: true,
            bytes_out_estimate: None,
            tls_info,
        })
    }
}

async fn connect_tcp_with_fallback(
    server_addrs: &[SocketAddr],
    connect_timeout: Duration,
) -> Result<(TcpStream, SocketAddr), RequestError> {
    let mut last_error: Option<RequestError> = None;

    for &server_addr in server_addrs {
        let socket = match server_addr {
            SocketAddr::V4(_) => TcpSocket::new_v4(),
            SocketAddr::V6(_) => TcpSocket::new_v6(),
        }
        .map_err(classify_io_error)?;

        let local_bind_addr = match server_addr {
            SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
            SocketAddr::V6(_) => SocketAddr::from(([0u16; 8], 0)),
        };

        if let Err(e) = socket.bind(local_bind_addr) {
            last_error = Some(classify_io_error(e));
            continue;
        }

        match tokio::time::timeout(connect_timeout, socket.connect(server_addr)).await {
            Ok(Ok(tcp)) => return Ok((tcp, server_addr)),
            Ok(Err(e)) => last_error = Some(classify_io_error(e)),
            Err(_) => {
                last_error = Some(RequestError::timeout(format!(
                    "TCP connect to {server_addr} timed out"
                )))
            }
        }
    }

    Err(last_error.unwrap_or_else(|| RequestError::connect("No resolved addresses to connect")))
}

fn estimate_h2_request_size(config: &RequestConfig) -> u64 {
    let mut size = 9u64; // HEADERS frame overhead
    size += config.method.as_str().len() as u64;
    size += config
        .url
        .path_and_query()
        .map_or(1, |pq| pq.as_str().len()) as u64;
    for (name, value) in &config.headers {
        size += (name.as_str().len() + value.as_bytes().len() + 2) as u64;
    }
    if let Some(ref body) = config.body {
        size += 9 + body.len() as u64; // DATA frame overhead + body
    }
    size
}

fn classify_h2_error<E: std::fmt::Display>(e: E) -> RequestError {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    if lower.contains("tls") || lower.contains("certificate") {
        RequestError::tls(msg)
    } else if lower.contains("timeout") {
        RequestError::timeout(msg)
    } else if lower.contains("connect")
        || lower.contains("refused")
        || lower.contains("dns")
        || lower.contains("resolve")
    {
        RequestError::connect(msg)
    } else {
        RequestError::http(msg)
    }
}

fn classify_io_error(e: std::io::Error) -> RequestError {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::AddrNotAvailable
        | std::io::ErrorKind::HostUnreachable
        | std::io::ErrorKind::NetworkUnreachable => RequestError::connect(e.to_string()),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            RequestError::timeout(e.to_string())
        }
        _ => RequestError::http(e.to_string()),
    }
}
