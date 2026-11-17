//! Tracks transferred stream payload bytes and reports them to proxy stats.

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ByteDirection {
    Rx,
    Tx,
    /// Counts the bytes only for `on_end`, without the global counters.
    Quiet,
}

/// The callback that a body calls with its data bytes when it ends or is dropped.
type OnEnd = Box<dyn FnOnce(u64) + Send + Sync>;

/// The data bytes of a body so far, reported once when the body goes away.
#[derive(Default)]
struct BodyEnd {
    sent: u64,
    on_end: Option<OnEnd>,
}

impl Drop for BodyEnd {
    fn drop(&mut self) {
        if let Some(on_end) = self.on_end.take() {
            on_end(self.sent);
        }
    }
}

pin_project! {
    pub struct ByteCountBody<B> {
        #[pin]
        inner: B,
        direction: ByteDirection,
        end: BodyEnd,
    }
}

impl<B> ByteCountBody<B> {
    pub fn rx(inner: B) -> Self {
        Self {
            inner,
            direction: ByteDirection::Rx,
            end: BodyEnd::default(),
        }
    }

    pub fn tx(inner: B) -> Self {
        Self {
            inner,
            direction: ByteDirection::Tx,
            end: BodyEnd::default(),
        }
    }

    /// A body whose bytes go only to `on_end`, for a counter that is no global one.
    pub fn quiet(inner: B) -> Self {
        Self {
            inner,
            direction: ByteDirection::Quiet,
            end: BodyEnd::default(),
        }
    }

    /// Calls `on_end` with the data bytes of the body when the body ends or is dropped.
    pub fn on_end(mut self, on_end: impl FnOnce(u64) + Send + Sync + 'static) -> Self {
        self.end.on_end = Some(Box::new(on_end));
        self
    }
}

impl<B> Body for ByteCountBody<B>
where
    B: Body<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        if let Some(Ok(ref frame)) = frame
            && let Some(data) = frame.data_ref()
        {
            let len = data.len() as u64;
            this.end.sent += len;
            if len > 0 {
                match this.direction {
                    ByteDirection::Rx => crate::stats::record_rx_bytes(len),
                    ByteDirection::Tx => crate::stats::record_tx_bytes(len),
                    ByteDirection::Quiet => {}
                }
            }
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
    use http_body_util::{BodyExt, Full};

    /// The byte counters are global to the process, so the tests that move bytes through a body
    /// run one after the other.
    static COUNTERS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn rx_and_tx_bodies_record_exact_bytes() {
        let _counters = COUNTERS.lock().await;
        let initial_rx = crate::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_rx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);
        let initial_tx = crate::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_tx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);

        let data = Bytes::from_static(b"hello world stream data");
        let rx_body = ByteCountBody::rx(Full::new(data.clone()));
        let collected_rx = rx_body.collect().await.unwrap().to_bytes();
        assert_eq!(collected_rx, data);

        let tx_body = ByteCountBody::tx(Full::new(data.clone()));
        let collected_tx = tx_body.collect().await.unwrap().to_bytes();
        assert_eq!(collected_tx, data);

        let final_rx = crate::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_rx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);
        let final_tx = crate::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_tx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);

        assert_eq!(final_rx - initial_rx, data.len() as u64);
        assert_eq!(final_tx - initial_tx, data.len() as u64);
    }

    /// A chunked response has no Content-Length, so the bytes that reached the client are known
    /// only when the body ends: the end reports the sum of every data frame, also when the
    /// client goes away before the last one.
    #[tokio::test]
    async fn the_end_of_a_body_reports_the_bytes_it_sent() {
        use std::sync::{Arc, Mutex};
        let _counters = COUNTERS.lock().await;
        let chunks = || {
            futures::stream::iter([b"hello ".as_slice(), b"chunked ", b"world"].map(|chunk| {
                Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from_static(chunk)))
            }))
        };
        let reported = Arc::new(Mutex::new(None));
        let sink = reported.clone();
        let body = ByteCountBody::tx(http_body_util::StreamBody::new(chunks()))
            .on_end(move |sent| *sink.lock().unwrap() = Some(sent));
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), 19);
        assert_eq!(*reported.lock().unwrap(), Some(19));

        let sink = reported.clone();
        let mut body = ByteCountBody::tx(http_body_util::StreamBody::new(chunks()))
            .on_end(move |sent| *sink.lock().unwrap() = Some(sent));
        body.frame().await.unwrap().unwrap();
        drop(body);
        assert_eq!(*reported.lock().unwrap(), Some(6));
    }
}
