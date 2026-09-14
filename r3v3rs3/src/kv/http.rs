//! A small HTTP/1.1 client for the APIs of Docker, etcd and Consul. It connects over a Unix
//! socket, TCP or TLS, and reads JSON bodies and line-delimited streams.

use anyhow::{anyhow, bail, Context as _};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::client::conn::http1;
use hyper::header::{HeaderValue, HOST, USER_AGENT};
use hyper::http::request::Builder;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use r3v3rs3_api::discovery::Endpoint;
use serde::de::DeserializeOwned;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use tracing::debug;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_SIZE: usize = 32 * 1024 * 1024;
const MAX_LINE_SIZE: usize = 1024 * 1024;
const MAX_ERROR_TEXT: usize = 200;

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

#[cfg(unix)]
async fn connect_unix(path: &str) -> anyhow::Result<Box<dyn Io>> {
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .with_context(|| format!("failed to connect to {path}"))?;
    Ok(Box::new(stream))
}

#[cfg(not(unix))]
async fn connect_unix(_path: &str) -> anyhow::Result<Box<dyn Io>> {
    bail!("Unix sockets are not available on this platform")
}

pub async fn read_json<T: DeserializeOwned>(response: Response<Incoming>) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&read_body(response).await?)?)
}

async fn read_body(response: Response<Incoming>) -> anyhow::Result<Bytes> {
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
