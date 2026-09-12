use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::rt::{Read, Write};
use hyper::Uri;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioIo;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use tower_service::Service;

use super::stream::MaybeHttpsStream;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A Connector for the `https` scheme.
#[derive(Clone)]
pub struct HttpsConnector<T> {
    force_https: bool,
    http: T,
    tls: TlsConnector,
}

impl HttpsConnector<HttpConnector> {
    /// Construct a new HttpsConnector.
    ///
    /// This uses hyper's default `HttpConnector`, and default `TlsConnector`.
    /// If you wish to use something besides the defaults, use `From::from`.
    ///
    /// # Note
    ///
    /// By default this connector will use plain HTTP if the URL provided uses
    /// the HTTP scheme (eg: http://example.com/).
    ///
    /// If you would like to force the use of HTTPS then call https_only(true)
    /// on the returned connector.
    ///
    /// # Panics
    ///
    /// This will panic if the underlying TLS context could not be created.
    ///
    /// To handle that error yourself, you can use the `HttpsConnector::from`
    /// constructor after trying to make a `TlsConnector`.
    pub fn new(config: Arc<ClientConfig>) -> Self {
        HttpsConnector::new_(tokio_rustls::TlsConnector::from(config))
    }

    fn new_(tls: TlsConnector) -> Self {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        HttpsConnector::from((http, tls))
    }
}

impl<T> From<(T, TlsConnector)> for HttpsConnector<T> {
    fn from(args: (T, TlsConnector)) -> HttpsConnector<T> {
        HttpsConnector {
            force_https: false,
            http: args.0,
            tls: args.1,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for HttpsConnector<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("HttpsConnector")
            .field("force_https", &self.force_https)
            .field("http", &self.http)
            .finish()
    }
}

impl<T> Service<Uri> for HttpsConnector<T>
where
    T: Service<Uri>,
    T::Response: Read + Write + Send + Unpin,
    T::Future: Send + 'static,
    T::Error: Into<BoxError>,
{
    type Response = MaybeHttpsStream<T::Response>;
    type Error = BoxError;
    type Future = HttpsConnecting<T::Response>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.http.poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let is_https = dst.scheme_str() == Some("https");
        // Early abort if HTTPS is forced but can't be used
        if !is_https && self.force_https {
            return err(ForceHttpsButUriNotHttps.into());
        }

        let host = dst
            .host()
            .unwrap_or("")
            .trim_matches(|c| c == '[' || c == ']')
            .to_owned();
        let connecting = self.http.call(dst);

        let tls_connector = self.tls.clone();

        let fut = async move {
            let tcp = connecting.await.map_err(Into::into)?;

            let maybe = if is_https {
                let stream = TokioIo::new(tcp);
                let server_name = ServerName::try_from(host.as_str())?.to_owned();
                let tls = TokioIo::new(tls_connector.connect(server_name, stream).await?);
                MaybeHttpsStream::Https(tls)
            } else {
                MaybeHttpsStream::Http(tcp)
            };
            Ok(maybe)
        };
        HttpsConnecting(Box::pin(fut))
    }
}

fn err<T>(e: BoxError) -> HttpsConnecting<T> {
    HttpsConnecting(Box::pin(async { Err(e) }))
}

type BoxedFut<T> = Pin<Box<dyn Future<Output = Result<MaybeHttpsStream<T>, BoxError>> + Send>>;

/// A Future representing work to connect to a URL, and a TLS handshake.
pub struct HttpsConnecting<T>(BoxedFut<T>);

impl<T: Read + Write + Unpin> Future for HttpsConnecting<T> {
    type Output = Result<MaybeHttpsStream<T>, BoxError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> fmt::Debug for HttpsConnecting<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("HttpsConnecting")
    }
}

// ===== Custom Errors =====

#[derive(Debug)]
struct ForceHttpsButUriNotHttps;

impl fmt::Display for ForceHttpsButUriNotHttps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("https required but URI was not https")
    }
}

impl std::error::Error for ForceHttpsButUriNotHttps {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_rustls::rustls::RootCertStore;

    #[tokio::test]
    async fn invalid_server_name_returns_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        let mut connector = HttpsConnector::new(Arc::new(config));

        // "127.1" resolves to 127.0.0.1 but is neither a valid DNS name nor an IP literal.
        let uri: Uri = format!("https://127.1:{port}/").parse().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), connector.call(uri))
            .await
            .unwrap();
        assert!(result.is_err());
    }
}
