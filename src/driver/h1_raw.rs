use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use super::{
    IpVersion, RequestConfig, RequestError, RequestResult, TlsInfo, maybe_enable_tcp_quickack,
    tls_info_from_rustls_conn,
};

#[derive(Debug)]
pub struct H1RawConnection {
    stream: Option<IoStream>,
    remote_addr: SocketAddr,
    authority: String,
    path_query: String,
    tls_info: TlsInfo,
    bytes_out_estimate: Option<u64>,
    request_head: Option<Vec<u8>>,
    read_buf: Vec<u8>,
    read_pos: usize,
}

#[derive(Debug)]
enum IoStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl IoStream {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            IoStream::Plain(s) => s.write_all(buf).await,
            IoStream::Tls(s) => s.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            IoStream::Plain(s) => s.flush().await,
            IoStream::Tls(s) => s.flush().await,
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            IoStream::Plain(s) => s.read(buf).await,
            IoStream::Tls(s) => s.read(buf).await,
        }
    }
}

impl H1RawConnection {
    pub fn tls_info(&self) -> &TlsInfo {
        &self.tls_info
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub fn prepare_request_template(&mut self, config: &RequestConfig) {
        if self.request_head.is_none() {
            self.request_head = Some(build_request_head(
                config,
                &self.authority,
                &self.path_query,
            ));
        }
        if self.bytes_out_estimate.is_none() {
            let head_len = self.request_head.as_ref().map_or(0, Vec::len);
            let body_len = config.body.as_ref().map_or(0, |b| b.len());
            self.bytes_out_estimate = Some((head_len + body_len) as u64);
        }
    }

    pub async fn send_request(
        &mut self,
        config: &RequestConfig,
    ) -> Result<RequestResult, RequestError> {
        self.prepare_request_template(config);
        let request_timeout = config.request_timeout;
        let start = Instant::now();
        let bytes_out = self
            .bytes_out_estimate
            .expect("raw h1 bytes_out estimate should be prepared");

        let fut = async {
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| RequestError::connect("raw h1 stream not connected"))?;

            let head = self
                .request_head
                .as_ref()
                .expect("raw h1 request head should be prepared");
            if let Some(body) = &config.body {
                // Single write: merge head + body to avoid extra syscall
                let mut combined = Vec::with_capacity(head.len() + body.len());
                combined.extend_from_slice(head);
                combined.extend_from_slice(body);
                stream
                    .write_all(&combined)
                    .await
                    .map_err(classify_io_error)?;
            } else {
                stream.write_all(head).await.map_err(classify_io_error)?;
            }
            stream.flush().await.map_err(classify_io_error)?;

            let (status, content_length, chunked, ttfb_us) = self.read_response_head(start).await?;
            let method_is_head = config.method == http::Method::HEAD;
            let has_no_body = method_is_head || status / 100 == 1 || status == 204 || status == 304;

            let bytes_in = if has_no_body {
                0
            } else if let Some(len) = content_length {
                self.read_content_length_body(len as usize, config.tail_friendly)
                    .await?
            } else if chunked {
                self.read_chunked_body(config.tail_friendly).await?
            } else {
                let bytes = self.read_to_eof_body(config.tail_friendly).await?;
                self.stream = None;
                bytes
            };

            let latency = start.elapsed();
            Ok::<RequestResult, RequestError>(RequestResult {
                status,
                latency_us: latency.as_micros() as u64,
                ttfb_us,
                bytes_in,
                bytes_out,
            })
        };

        tokio::time::timeout(request_timeout, fut)
            .await
            .map_err(|_| RequestError::timeout("Request timed out"))?
    }

    async fn read_response_head(
        &mut self,
        start: Instant,
    ) -> Result<(u16, Option<u64>, bool, u64), RequestError> {
        let mut ttfb_us = None;
        let head_end = loop {
            if let Some(pos) = find_headers_end(self.buffered()) {
                break pos;
            }
            let n = self.read_more().await?;
            if n == 0 {
                return Err(RequestError::connect(
                    "Connection closed while reading response headers",
                ));
            }
            if ttfb_us.is_none() {
                ttfb_us = Some(start.elapsed().as_micros() as u64);
            }
        };
        let ttfb_us = ttfb_us.unwrap_or_else(|| start.elapsed().as_micros() as u64);

        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut res = httparse::Response::new(&mut headers);
        let parsed = res
            .parse(&self.buffered()[..head_end])
            .map_err(|e| RequestError::http(format!("Failed to parse response headers: {e}")))?;
        if !parsed.is_complete() {
            return Err(RequestError::http(
                "Incomplete response headers after delimiter",
            ));
        }

        let status = res
            .code
            .ok_or_else(|| RequestError::http("Missing response status code"))?;

        let mut content_length = None;
        let mut chunked = false;
        for h in res.headers {
            if h.name.eq_ignore_ascii_case("content-length") {
                if let Ok(v) = std::str::from_utf8(h.value)
                    && let Ok(n) = v.trim().parse::<u64>()
                {
                    content_length = Some(n);
                }
            } else if h.name.eq_ignore_ascii_case("transfer-encoding")
                && contains_chunked_token(h.value)
            {
                chunked = true;
            }
        }

        self.consume(head_end);
        Ok((status, content_length, chunked, ttfb_us))
    }

    async fn read_content_length_body(
        &mut self,
        mut remaining: usize,
        tail_friendly: bool,
    ) -> Result<u64, RequestError> {
        let mut bytes_in = 0u64;
        let buffered = self.buffered().len();
        if buffered > 0 {
            let take = remaining.min(buffered);
            bytes_in += take as u64;
            remaining -= take;
            self.consume(take);
        }

        let mut tmp = [0u8; 16 * 1024];
        let mut chunks = 0u32;
        while remaining > 0 {
            let max_read = remaining.min(tmp.len());
            let n = self.read_stream(&mut tmp[..max_read]).await?;
            if n == 0 {
                return Err(RequestError::connect(
                    "Connection closed while reading response body",
                ));
            }
            bytes_in += n as u64;
            remaining -= n;
            if tail_friendly {
                chunks = chunks.wrapping_add(1);
                if chunks.is_multiple_of(4) {
                    tokio::task::yield_now().await;
                }
            }
        }
        Ok(bytes_in)
    }

    async fn read_chunked_body(&mut self, tail_friendly: bool) -> Result<u64, RequestError> {
        let mut bytes_in = 0u64;
        let mut chunks = 0u32;
        loop {
            let line_end = self.read_until_crlf().await?;
            let line = &self.buffered()[..line_end];
            let chunk_size = parse_chunk_size(line)?;
            self.consume(line_end + 2); // line + CRLF

            if chunk_size == 0 {
                // Consume trailers until empty line.
                loop {
                    let trailer_end = self.read_until_crlf().await?;
                    self.consume(trailer_end + 2);
                    if trailer_end == 0 {
                        break;
                    }
                }
                break;
            }

            self.ensure_buffer(chunk_size + 2).await?;
            let trailer_ok = &self.buffered()[chunk_size..chunk_size + 2] == b"\r\n";
            if !trailer_ok {
                return Err(RequestError::http("Invalid chunk terminator"));
            }
            bytes_in += chunk_size as u64;
            self.consume(chunk_size + 2);
            if tail_friendly {
                chunks = chunks.wrapping_add(1);
                if chunks.is_multiple_of(4) {
                    tokio::task::yield_now().await;
                }
            }
        }
        Ok(bytes_in)
    }

    async fn read_to_eof_body(&mut self, tail_friendly: bool) -> Result<u64, RequestError> {
        let mut bytes_in = self.buffered().len() as u64;
        self.read_buf.clear();
        self.read_pos = 0;

        let mut tmp = [0u8; 16 * 1024];
        let mut chunks = 0u32;
        loop {
            let n = self.read_stream(&mut tmp).await?;
            if n == 0 {
                break;
            }
            bytes_in += n as u64;
            if tail_friendly {
                chunks = chunks.wrapping_add(1);
                if chunks.is_multiple_of(4) {
                    tokio::task::yield_now().await;
                }
            }
        }
        Ok(bytes_in)
    }

    async fn read_until_crlf(&mut self) -> Result<usize, RequestError> {
        loop {
            if let Some(pos) = find_crlf(self.buffered()) {
                return Ok(pos);
            }
            let n = self.read_more().await?;
            if n == 0 {
                return Err(RequestError::connect(
                    "Connection closed while reading chunked line",
                ));
            }
        }
    }

    async fn ensure_buffer(&mut self, n: usize) -> Result<(), RequestError> {
        while self.buffered().len() < n {
            let got = self.read_more().await?;
            if got == 0 {
                return Err(RequestError::connect(
                    "Connection closed while reading chunked body",
                ));
            }
        }
        Ok(())
    }

    async fn read_more(&mut self) -> Result<usize, RequestError> {
        let mut tmp = [0u8; 16 * 1024];
        let n = self.read_stream(&mut tmp).await?;
        if n > 0 {
            self.read_buf.extend_from_slice(&tmp[..n]);
        }
        Ok(n)
    }

    async fn read_stream(&mut self, buf: &mut [u8]) -> Result<usize, RequestError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| RequestError::connect("raw h1 stream not connected"))?;
        stream.read(buf).await.map_err(classify_io_error)
    }

    #[inline]
    fn buffered(&self) -> &[u8] {
        &self.read_buf[self.read_pos..]
    }

    #[inline]
    fn consume(&mut self, n: usize) {
        self.read_pos += n;
        self.compact_if_needed();
    }

    #[inline]
    fn compact_if_needed(&mut self) {
        if self.read_pos == self.read_buf.len() {
            self.read_buf.clear();
            self.read_pos = 0;
            return;
        }

        // Avoid per-request drain/memmove; compact only occasionally.
        if self.read_pos >= 32 * 1024 || self.read_pos * 2 >= self.read_buf.len() {
            self.read_buf.drain(..self.read_pos);
            self.read_pos = 0;
        }
    }
}

pub struct H1RawFactory {
    tls_config: Arc<rustls::ClientConfig>,
    server_addrs: Vec<SocketAddr>,
    server_name: String,
    authority: String,
    path_query: String,
    connect_timeout: Duration,
    tcp_quickack: bool,
    use_tls: bool,
}

impl H1RawFactory {
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
        let scheme = url.scheme_str().unwrap_or("http");
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

        let authority = url
            .authority()
            .map(|a| a.as_str().to_string())
            .unwrap_or_else(|| format!("{host}:{port}"));
        let path_query = url
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());

        Ok(Self {
            tls_config,
            server_addrs: addrs,
            server_name: host.to_string(),
            authority,
            path_query,
            connect_timeout,
            tcp_quickack,
            use_tls: scheme.eq_ignore_ascii_case("https"),
        })
    }

    pub async fn connect(&self) -> Result<H1RawConnection, RequestError> {
        let (tcp, remote_addr) =
            connect_tcp_with_fallback(&self.server_addrs, self.connect_timeout).await?;
        let _ = tcp.set_nodelay(true);
        maybe_enable_tcp_quickack(&tcp, self.tcp_quickack);

        let (stream, tls_info) = if self.use_tls {
            let connector = TlsConnector::from(self.tls_config.clone());
            let server_name = ServerName::try_from(self.server_name.clone())
                .map_err(|e| RequestError::tls(format!("Invalid TLS server name: {e}")))?;
            let tls_stream =
                tokio::time::timeout(self.connect_timeout, connector.connect(server_name, tcp))
                    .await
                    .map_err(|_| RequestError::timeout("TLS handshake timed out"))?
                    .map_err(|e| RequestError::tls(format!("TLS handshake failed: {e}")))?;
            let info = tls_info_from_rustls_conn(tls_stream.get_ref().1);
            (IoStream::Tls(Box::new(tls_stream)), info)
        } else {
            (IoStream::Plain(tcp), TlsInfo::default())
        };

        Ok(H1RawConnection {
            stream: Some(stream),
            remote_addr,
            authority: self.authority.clone(),
            path_query: self.path_query.clone(),
            tls_info,
            bytes_out_estimate: None,
            request_head: None,
            read_buf: Vec::with_capacity(64 * 1024),
            read_pos: 0,
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

fn build_request_head(config: &RequestConfig, authority: &str, path_query: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(512 + config.headers.len() * 48);
    out.extend_from_slice(config.method.as_str().as_bytes());
    out.extend_from_slice(b" ");
    out.extend_from_slice(path_query.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\n");

    let mut has_host = false;
    let mut has_content_length = false;
    for (name, value) in &config.headers {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host") {
            has_host = true;
        } else if n.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        out.extend_from_slice(n.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }

    if !has_host {
        out.extend_from_slice(b"Host: ");
        out.extend_from_slice(authority.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if !has_content_length && let Some(body) = &config.body {
        out.extend_from_slice(b"Content-Length: ");
        out.extend_from_slice(body.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
    }

    out.extend_from_slice(b"\r\n");
    out
}

fn parse_chunk_size(line: &[u8]) -> Result<usize, RequestError> {
    let size_part = line.split(|b| *b == b';').next().unwrap_or(&[]);
    let size_str = std::str::from_utf8(size_part)
        .map_err(|e| RequestError::http(format!("Invalid chunk size utf8: {e}")))?;
    usize::from_str_radix(size_str.trim(), 16)
        .map_err(|e| RequestError::http(format!("Invalid chunk size: {e}")))
}

fn contains_chunked_token(value: &[u8]) -> bool {
    // Case-insensitive token scan without temporary allocation.
    value.windows(7).any(|w| w.eq_ignore_ascii_case(b"chunked"))
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
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
        _ => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("tls") || msg.contains("certificate") || msg.contains("ssl") {
                RequestError::tls(e.to_string())
            } else {
                RequestError::http(e.to_string())
            }
        }
    }
}
