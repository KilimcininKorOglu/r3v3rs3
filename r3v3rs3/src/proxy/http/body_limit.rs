//! Limits the size of a request body that r3v3rs3 sends to an upstream server.

use super::compression::header_number;
use super::error::ProxyError;
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::Request;
use hyper::body::{Body, Frame, SizeHint};
use hyper::header::{CONTENT_LENGTH, HeaderMap};
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};

type ProxyBody = BoxBody<Bytes, anyhow::Error>;

/// True when the `Content-Length` header of the request is larger than `limit`. `0` has no limit.
pub fn declared_too_large(headers: &HeaderMap, limit: u64) -> bool {
    limit > 0 && header_number(headers, CONTENT_LENGTH).is_some_and(|length| length > limit)
}

/// Records that a limited body passed its limit.
#[derive(Debug, Default, Clone)]
pub struct BodyLimit(Option<Arc<AtomicBool>>);

impl BodyLimit {
    /// Wraps the request body, so it fails after `limit` bytes. `0` leaves the body unchanged.
    pub fn apply(req: Request<ProxyBody>, limit: u64) -> (Request<ProxyBody>, Self) {
        if limit == 0 {
            return (req, Self(None));
        }
        let exceeded = Arc::new(AtomicBool::new(false));
        let req = req.map(|inner| {
            BoxBody::new(LimitedBody {
                inner,
                remaining: limit,
                exceeded: exceeded.clone(),
            })
        });
        (req, Self(Some(exceeded)))
    }

    /// True when the body failed because it passed the limit.
    pub fn exceeded(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|exceeded| exceeded.load(Ordering::Relaxed))
    }
}

pin_project! {
    struct LimitedBody {
        #[pin]
        inner: ProxyBody,
        remaining: u64,
        exceeded: Arc<AtomicBool>,
    }
}

impl Body for LimitedBody {
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, anyhow::Error>>> {
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        let length = match &frame {
            Some(Ok(frame)) => frame.data_ref().map_or(0, |data| data.len() as u64),
            _ => 0,
        };
        if length > *this.remaining {
            this.exceeded.store(true, Ordering::Relaxed);
            return Poll::Ready(Some(Err(ProxyError::PayloadTooLarge.into())));
        }
        *this.remaining -= length;
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
    use http_body_util::{BodyExt, Full, StreamBody};

    fn chunked(chunks: &[&'static str]) -> ProxyBody {
        let frames = chunks
            .iter()
            .map(|chunk| Ok::<_, anyhow::Error>(Frame::data(Bytes::from_static(chunk.as_bytes()))))
            .collect::<Vec<_>>();
        BoxBody::new(StreamBody::new(futures::stream::iter(frames)))
    }

    #[tokio::test]
    async fn a_body_fails_after_the_limit() {
        let (req, limit) = BodyLimit::apply(Request::new(chunked(&["abc", "de"])), 5);
        assert_eq!(req.into_body().collect().await.unwrap().to_bytes(), "abcde");
        assert!(!limit.exceeded());

        let (req, limit) = BodyLimit::apply(Request::new(chunked(&["abc", "def"])), 5);
        let err = req.into_body().collect().await.unwrap_err();
        assert!(matches!(
            err.downcast_ref::<ProxyError>(),
            Some(ProxyError::PayloadTooLarge)
        ));
        assert!(limit.exceeded());

        let full = BoxBody::new(Full::new(Bytes::from_static(b"abcdef")).map_err(Into::into));
        let (req, limit) = BodyLimit::apply(Request::new(full), 0);
        assert_eq!(req.body().size_hint().exact(), Some(6));
        assert_eq!(
            req.into_body().collect().await.unwrap().to_bytes(),
            "abcdef"
        );
        assert!(!limit.exceeded());
    }

    #[test]
    fn a_declared_length_is_checked_against_the_limit() {
        let mut headers = HeaderMap::new();
        assert!(!declared_too_large(&headers, 5));
        headers.insert(CONTENT_LENGTH, "6".parse().unwrap());
        assert!(declared_too_large(&headers, 5));
        assert!(!declared_too_large(&headers, 6));
        assert!(!declared_too_large(&headers, 0));
    }
}
