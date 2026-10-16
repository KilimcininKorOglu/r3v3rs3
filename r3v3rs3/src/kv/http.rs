//! A small HTTP/1.1 client for the APIs of Docker, etcd and Consul. It connects over a Unix
//! socket, TCP or TLS, and reads JSON bodies and line-delimited streams.

use anyhow::{Context as _, anyhow, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::header::{HOST, HeaderValue, USER_AGENT};
use hyper::http::request::Builder;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use r3v3rs3_api::discovery::Endpoint;
use serde::de::DeserializeOwned;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::debug;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_SIZE: usize = 32 * 1024 * 1024;
const MAX_LINE_SIZE: usize = 1024 * 1024;
const MAX_ERROR_TEXT: usize = 200;
/// The largest head of an upgraded response.
const MAX_HEADER_SIZE: usize = 8 * 1024;

/// The end of the response headers of an upgraded connection.
fn window(data: &[u8]) -> &[u8] {
    data.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(&[], |index| &data[..index + 4])
}

fn window_ends(data: &[u8]) -> bool {
    !window(data).is_empty()
}

/// The duplex stream of an upgraded connection.
pub(crate) type UpgradedStream = PrefixedIo;

/// A stream that replays the bytes that the header read already consumed before the rest.
pub struct PrefixedIo {
    prefix: Bytes,
    io: Box<dyn Io>,
}

impl AsyncRead for PrefixedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.prefix.is_empty() {
            Pin::new(&mut self.io).poll_read(cx, buf)
        } else {
            let size = self.prefix.len().min(buf.remaining());
            let data = self.prefix.split_to(size);
            buf.put_slice(&data);
            Poll::Ready(Ok(()))
        }
    }
}

impl AsyncWrite for PrefixedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

trait Io: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Io for T {}

#[derive(Clone)]
pub struct ApiClient {
    endpoints: Arc<[Endpoint]>,
    /// The index of the endpoint that accepted the last connection.
    current: Arc<AtomicUsize>,
    tls: TlsConnector,
}

impl ApiClient {
    /// A connection tries the endpoints in turn, starting with the endpoint that accepted the
    /// last connection. `tls` is used only by an `https` endpoint.
    pub fn new(endpoints: Vec<Endpoint>, tls: Arc<ClientConfig>) -> Self {
        Self {
            endpoints: endpoints.into(),
            current: Arc::default(),
            tls: TlsConnector::from(tls),
        }
    }

    /// Starts a request to a path of the API, with the `User-Agent` header. The request gets the
    /// `Host` header of the endpoint that it is sent to.
    pub fn builder(&self, method: Method, path: &str) -> Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header(USER_AGENT, concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")))
    }

    pub fn get(&self, path: &str) -> anyhow::Result<Request<Full<Bytes>>> {
        Ok(self
            .builder(Method::GET, path)
            .body(Full::new(Bytes::new()))?)
    }

    /// Sends the request on a new connection and returns the response when its status is a
    /// success. The body is not read, so a stream can stay open.
    pub async fn send(&self, request: Request<Full<Bytes>>) -> anyhow::Result<Response<Incoming>> {
        self.send_with(request, RESPONSE_TIMEOUT, &[]).await
    }

    /// Sends a request that switches the connection to a raw duplex stream, like the Docker
    /// exec hijack. The request goes over its own connection without hyper, the `101` response
    /// headers are read by hand, and the rest of the stream is returned.
    pub async fn send_upgrade(
        &self,
        request: Request<Full<Bytes>>,
    ) -> anyhow::Result<UpgradedStream> {
        let (mut io, endpoint) = self.connect().await?;
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let body = request.into_body().into_inner().unwrap_or_default();
        let head = format!(
            "{} {} HTTP/1.1\r\nhost: {}\r\nuser-agent: {}\r\nconnection: Upgrade\r\nupgrade: tcp\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            method,
            path,
            host_header(endpoint),
            concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")),
            body.len(),
        );
        let mut wire = head.into_bytes();
        wire.extend_from_slice(&body);
        io.write_all(&wire).await?;
        io.flush().await?;

        let mut buffered = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            if window_ends(&buffered) {
                break;
            }
            let size = tokio::time::timeout(RESPONSE_TIMEOUT, io.read(&mut chunk)).await??;
            if size == 0 {
                bail!("the connection closed before the upgrade");
            }
            buffered.extend_from_slice(&chunk[..size]);
            if buffered.len() > MAX_HEADER_SIZE {
                bail!("the response headers are too large");
            }
        }
        let split = window(&buffered).len();
        let head = String::from_utf8_lossy(&buffered[..split]);
        let status = head.lines().next().unwrap_or_default();
        if !status.contains(" 101 ") {
            bail!("the connection was not upgraded: {}", status.trim());
        }
        Ok(PrefixedIo {
            prefix: Bytes::copy_from_slice(&buffered[split..]),
            io,
        })
    }

    /// Sends the request and waits for the response headers up to `timeout`. A status in
    /// `allowed` is not an error.
    pub async fn send_with(
        &self,
        mut request: Request<Full<Bytes>>,
        timeout: Duration,
        allowed: &[StatusCode],
    ) -> anyhow::Result<Response<Incoming>> {
        let (io, endpoint) = self.connect().await?;
        let host = HeaderValue::from_str(&host_header(endpoint))?;
        request.headers_mut().insert(HOST, host);
        let (mut sender, connection) = http1::handshake(TokioIo::new(io)).await?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                debug!(%err, "an API connection failed");
            }
        });
        let response = tokio::time::timeout(timeout, sender.send_request(request))
            .await
            .map_err(|_| anyhow!("the response timed out"))??;
        let status = response.status();
        if !status.is_success() && !allowed.contains(&status) {
            let body = read_body(response).await.unwrap_or_default();
            let text = String::from_utf8_lossy(&body[..body.len().min(MAX_ERROR_TEXT)]);
            bail!("unexpected status {status}: {}", text.trim());
        }
        Ok(response)
    }

    /// Connects to the first endpoint that accepts a connection, and returns the connection and
    /// the endpoint.
    async fn connect(&self) -> anyhow::Result<(Box<dyn Io>, &Endpoint)> {
        let count = self.endpoints.len();
        let start = self.current.load(Ordering::Relaxed);
        let mut last_error = anyhow!("no endpoint is set");
        for index in (0..count).map(|offset| (start + offset) % count) {
            let endpoint = &self.endpoints[index];
            match tokio::time::timeout(CONNECT_TIMEOUT, self.connect_to(endpoint)).await {
                Ok(Ok(io)) => {
                    self.current.store(index, Ordering::Relaxed);
                    return Ok((io, endpoint));
                }
                Ok(Err(err)) => last_error = err,
                Err(_) => last_error = anyhow!("the connection timed out"),
            }
        }
        Err(last_error)
    }

    async fn connect_to(&self, endpoint: &Endpoint) -> anyhow::Result<Box<dyn Io>> {
        match endpoint {
            Endpoint::Unix(path) => connect_unix(path).await,
            Endpoint::Tcp { tls, host, port } => {
                let tcp = TcpStream::connect((host.as_str(), *port))
                    .await
                    .with_context(|| format!("failed to connect to {host}:{port}"))?;
                let stream: Box<dyn Io> = if *tls {
                    let name = ServerName::try_from(host.clone())?;
                    Box::new(self.tls.connect(name, tcp).await?)
                } else {
                    Box::new(tcp)
                };
                Ok(stream)
            }
        }
    }
}

fn host_header(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::Unix(_) => "localhost".to_string(),
        Endpoint::Tcp { host, port, .. } if host.contains(':') => format!("[{host}]:{port}"),
        Endpoint::Tcp { host, port, .. } => format!("{host}:{port}"),
    }
}

async fn connect_unix(path: &str) -> anyhow::Result<Box<dyn Io>> {
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .with_context(|| format!("failed to connect to {path}"))?;
    Ok(Box::new(stream))
}

pub async fn read_json<T: DeserializeOwned>(response: Response<Incoming>) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&read_body(response).await?)?)
}

pub async fn read_body(response: Response<Incoming>) -> anyhow::Result<Bytes> {
    let body = Limited::new(response.into_body(), MAX_BODY_SIZE).collect();
    let body = tokio::time::timeout(RESPONSE_TIMEOUT, body)
        .await
        .map_err(|_| anyhow!("the response body timed out"))?
        .map_err(|err| anyhow!(err))?;
    Ok(body.to_bytes())
}

/// Reads a response body line by line.
pub struct Lines {
    body: Incoming,
    buffer: Vec<u8>,
}

impl Lines {
    pub fn new(response: Response<Incoming>) -> Self {
        Self {
            body: response.into_body(),
            buffer: Vec::new(),
        }
    }

    /// Returns the next line without its newline, or `None` when the stream ends. The future can
    /// be dropped and called again without losing data.
    pub async fn next(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line = self.buffer.drain(..=end).collect::<Vec<_>>();
                line.pop();
                return Ok(Some(line));
            }
            if self.buffer.len() > MAX_LINE_SIZE {
                bail!("a line of the stream is too long");
            }
            let Some(frame) = self.body.frame().await else {
                return Ok(None);
            };
            if let Ok(data) = frame?.into_data() {
                self.buffer.extend_from_slice(&data);
            }
        }
    }
}
