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
}

pin_project! {
    pub struct ByteCountBody<B> {
        #[pin]
        inner: B,
        direction: ByteDirection,
    }
}

impl<B> ByteCountBody<B> {
    pub fn rx(inner: B) -> Self {
        Self {
            inner,
            direction: ByteDirection::Rx,
        }
    }

    pub fn tx(inner: B) -> Self {
        Self {
            inner,
            direction: ByteDirection::Tx,
        }
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
            if len > 0 {
                match this.direction {
                    ByteDirection::Rx => crate::proxy::stats::record_rx_bytes(len),
                    ByteDirection::Tx => crate::proxy::stats::record_tx_bytes(len),
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

    #[tokio::test]
    async fn rx_and_tx_bodies_record_exact_bytes() {
        let initial_rx = crate::proxy::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_rx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);
        let initial_tx = crate::proxy::stats::snapshot()
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

        let final_rx = crate::proxy::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_rx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);
        let final_tx = crate::proxy::stats::snapshot()
            .into_iter()
            .find(|(k, _)| *k == "r3v3rs3_tx_bytes_total")
            .map(|(_, v)| v)
            .unwrap_or(0);

        assert_eq!(final_rx - initial_rx, data.len() as u64);
        assert_eq!(final_tx - initial_tx, data.len() as u64);
    }
}
