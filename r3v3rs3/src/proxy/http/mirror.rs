//! Sends a copy of the requests of a route to its mirror servers.

use super::pool::{ConnectionPool, UpstreamH2c};
use bytes::{Bytes, BytesMut};
use futures::future::join_all;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Body, Frame, SizeHint};
use hyper::header::{HOST, HeaderValue, UPGRADE};
use hyper::{Request, Uri};
use pin_project_lite::pin_project;
use r3v3rs3_api::{mirror, proxy::Server};
use rand::RngExt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tracing::debug;

type ProxyBody = BoxBody<Bytes, anyhow::Error>;

/// The largest number of copies of one route in progress.
const MAX_COPIES: usize = 64;

/// The mirror servers of a route.
#[derive(Debug)]
pub struct Mirror {
    pub servers: Arc<[Server]>,
    percent: u8,
    max_body_size: u64,
    permits: Arc<Semaphore>,
    pool: Arc<ConnectionPool>,
    /// `Duration::ZERO` disables the limit.
    request_timeout: Duration,
}

impl Mirror {
    pub fn new(
        config: &mirror::Mirror,
        pool: Arc<ConnectionPool>,
        request_timeout: Duration,
    ) -> Self {
        Self {
            servers: config.servers.clone().into(),
            percent: config.percent,
            max_body_size: config.max_body_size,
            permits: Arc::new(Semaphore::new(MAX_COPIES)),
            pool,
            request_timeout,
        }
    }

    /// Takes the permit of a copy. `None` when the route has no mirror server, the sample skips
    /// the request, the request is an upgrade, its body is longer than the limit, or every permit
    /// is in use.
    pub fn admit<B: Body>(&self, req: &Request<B>) -> Option<OwnedSemaphorePermit> {
        let skipped = self.servers.is_empty()
            || !self.sampled()
            || req.headers().contains_key(UPGRADE)
            || req.body().size_hint().lower() > self.max_body_size;
        if skipped {
            return None;
        }
        self.permits.clone().try_acquire_owned().ok()
    }

    fn sampled(&self) -> bool {
        self.percent >= 100 || rand::rng().random_range(0..100) < self.percent
    }

    /// Returns the request with a body that keeps a copy of itself. When the body ends within the
    /// limit, a task sends the copy to each URI and drops the responses. The task does not delay
    /// the request.
    pub fn tee(
        &self,
        req: Request<ProxyBody>,
        uris: Vec<Uri>,
        permit: OwnedSemaphorePermit,
    ) -> Request<ProxyBody> {
        let mut head = Request::new(());
        *head.method_mut() = req.method().clone();
        *head.headers_mut() = req.headers().clone();
        if let Some(h2c) = req.extensions().get::<UpstreamH2c>() {
            head.extensions_mut().insert(*h2c);
        }
        let (sender, receiver) = oneshot::channel();
        let copies = Copies {
            head,
            uris,
            pool: self.pool.clone(),
            request_timeout: self.request_timeout,
        };
        tokio::spawn(copies.send(receiver, permit));
        if req.body().is_end_stream() {
            let _ = sender.send(Bytes::new());
            return req;
        }
        let limit = usize::try_from(self.max_body_size).unwrap_or(usize::MAX);
        req.map(|inner| {
            BoxBody::new(TeeBody {
                inner,
                copy: BytesMut::new(),
                limit,
                sender: Some(sender),
            })
        })
    }
}

/// The copies of one request.
struct Copies {
    head: Request<()>,
    uris: Vec<Uri>,
    pool: Arc<ConnectionPool>,
    request_timeout: Duration,
}

impl Copies {
    /// Waits for the body, then sends a copy to each URI. A body that fails, passes the limit or
    /// does not end sends nothing.
    async fn send(self, body: oneshot::Receiver<Bytes>, _permit: OwnedSemaphorePermit) {
        let Ok(body) = body.await else {
            return;
        };
        let copies = &self;
        let sending = copies.uris.iter().map(|uri| {
            let req = copies.request(uri.clone(), body.clone());
            async move {
                let result = copies
                    .pool
                    .send_discarded(req, copies.request_timeout)
                    .await;
                if let Err(err) = result {
                    debug!(%err, "the mirror server did not answer");
                }
            }
        });
        join_all(sending).await;
    }

    fn request(&self, uri: Uri, body: Bytes) -> Request<ProxyBody> {
        let mut req = Request::new(BoxBody::new(
            Full::new(body).map_err(|never| match never {}),
        ));
        *req.method_mut() = self.head.method().clone();
        *req.headers_mut() = self.head.headers().clone();
        if let Some(host) = uri
            .authority()
            .and_then(|host| HeaderValue::from_str(host.as_str()).ok())
        {
            req.headers_mut().insert(HOST, host);
        }
        if let Some(h2c) = self.head.extensions().get::<UpstreamH2c>() {
            req.extensions_mut().insert(*h2c);
        }
        *req.uri_mut() = uri;
        req
    }
}

pin_project! {
    struct TeeBody {
        #[pin]
        inner: ProxyBody,
        copy: BytesMut,
        limit: usize,
        // `None` after the copy was sent or dropped.
        sender: Option<oneshot::Sender<Bytes>>,
    }
}

impl Body for TeeBody {
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, anyhow::Error>>> {
        let mut this = self.project();
        let frame = ready!(this.inner.as_mut().poll_frame(cx));
        match &frame {
            Some(Ok(frame)) => {
                let data = frame.data_ref().map_or(&[][..], |data| data.as_ref());
                if this.copy.len() + data.len() > *this.limit {
                    this.sender.take();
                } else if this.sender.is_some() {
                    this.copy.extend_from_slice(data);
                }
            }
            Some(Err(_)) => {
                this.sender.take();
            }
            None => {}
        }
        if (frame.is_none() || this.inner.is_end_stream())
            && let Some(sender) = this.sender.take()
        {
            let _ = sender.send(this.copy.split().freeze());
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::StreamBody;

    fn tee(chunks: &[&'static str], limit: usize) -> (TeeBody, oneshot::Receiver<Bytes>) {
        let frames = chunks
            .iter()
            .map(|chunk| Ok::<_, anyhow::Error>(Frame::data(Bytes::from_static(chunk.as_bytes()))))
            .collect::<Vec<_>>();
        let (sender, receiver) = oneshot::channel();
        let body = TeeBody {
            inner: BoxBody::new(StreamBody::new(futures::stream::iter(frames))),
            copy: BytesMut::new(),
            limit,
            sender: Some(sender),
        };
        (body, receiver)
    }

    #[tokio::test]
    async fn the_copy_is_sent_only_for_a_whole_body_within_the_limit() {
        let (body, receiver) = tee(&["abc", "de"], 5);
        assert_eq!(body.collect().await.unwrap().to_bytes(), "abcde");
        assert_eq!(receiver.await.unwrap(), "abcde");

        let (body, receiver) = tee(&["abc", "def"], 5);
        assert_eq!(body.collect().await.unwrap().to_bytes(), "abcdef");
        assert!(receiver.await.is_err());

        let (mut body, receiver) = tee(&["abc", "de"], 5);
        body.frame().await.unwrap().unwrap();
        drop(body);
        assert!(receiver.await.is_err());
    }
}
