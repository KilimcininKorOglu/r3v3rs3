use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::traffic::{ActiveConnectionInfo, ConnectionProtocol};
use std::collections::BTreeMap;
use std::io::Result as IoResult;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::task::{Context, Poll};
use std::time::Instant;
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::watch;

struct ConnectionEntry {
    id: ShortId,
    client_addr: String,
    proxy_id: Option<ShortId>,
    proxy_name: String,
    protocol: ConnectionProtocol,
    upstream_addr: String,
    started_at: OffsetDateTime,
    started_instant: Instant,
    bytes_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    terminate_tx: watch::Sender<bool>,
}

struct ConnectionTrackerStore {
    connections: RwLock<BTreeMap<ShortId, ConnectionEntry>>,
}

static STORE: LazyLock<ConnectionTrackerStore> = LazyLock::new(|| ConnectionTrackerStore {
    connections: RwLock::new(BTreeMap::new()),
});

/// RAII guard that automatically unregisters an active stream upon drop.
pub struct StreamGuard {
    id: ShortId,
}

impl StreamGuard {
    /// Returns the unique ID of the tracked stream.
    pub fn id(&self) -> ShortId {
        self.id
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        unregister(&self.id);
    }
}

/// Registers a new active stream or connection and returns its registration assets.
pub fn register(
    proxy_id: Option<ShortId>,
    proxy_name: impl Into<String>,
    protocol: ConnectionProtocol,
    client_addr: impl Into<String>,
    upstream_addr: impl Into<String>,
) -> (
    ShortId,
    StreamGuard,
    watch::Receiver<bool>,
    Arc<AtomicU64>,
    Arc<AtomicU64>,
) {
    let id = ShortId::new();
    let (terminate_tx, terminate_rx) = watch::channel(false);
    let bytes_sent = Arc::new(AtomicU64::new(0));
    let bytes_received = Arc::new(AtomicU64::new(0));

    let entry = ConnectionEntry {
        id,
        client_addr: client_addr.into(),
        proxy_id,
        proxy_name: proxy_name.into(),
        protocol,
        upstream_addr: upstream_addr.into(),
        started_at: OffsetDateTime::now_utc(),
        started_instant: Instant::now(),
        bytes_sent: Arc::clone(&bytes_sent),
        bytes_received: Arc::clone(&bytes_received),
        terminate_tx,
    };

    if let Ok(mut conns) = STORE.connections.write() {
        conns.insert(id, entry);
    }

    (
        id,
        StreamGuard { id },
        terminate_rx,
        bytes_sent,
        bytes_received,
    )
}

/// Unregisters an active connection by ID.
pub fn unregister(id: &ShortId) {
    if let Ok(mut conns) = STORE.connections.write() {
        conns.remove(id);
    }
}

/// Lists all active connections matching the optional proxy filter.
pub fn list(filter_proxy_id: Option<&ShortId>) -> Vec<ActiveConnectionInfo> {
    let Ok(conns) = STORE.connections.read() else {
        return Vec::new();
    };

    conns
        .values()
        .filter(|c| filter_proxy_id.is_none() || c.proxy_id.as_ref() == filter_proxy_id)
        .map(entry_to_info)
        .collect()
}

fn entry_to_info(entry: &ConnectionEntry) -> ActiveConnectionInfo {
    let uptime_secs = entry.started_instant.elapsed().as_secs();
    let sent = entry.bytes_sent.load(Ordering::Relaxed);
    let received = entry.bytes_received.load(Ordering::Relaxed);
    let total_bytes = sent.saturating_add(received);
    let current_rate_bps = total_bytes.checked_div(uptime_secs).unwrap_or(total_bytes);

    ActiveConnectionInfo {
        id: entry.id,
        client_addr: entry.client_addr.clone(),
        proxy_id: entry.proxy_id,
        proxy_name: entry.proxy_name.clone(),
        protocol: entry.protocol,
        upstream_addr: entry.upstream_addr.clone(),
        started_at: entry.started_at,
        uptime_secs,
        bytes_sent: sent,
        bytes_received: received,
        current_rate_bps,
    }
}

/// Terminates an active connection by its ShortId.
pub fn terminate(id: ShortId) -> Result<(), Error> {
    let Ok(conns) = STORE.connections.read() else {
        return Err(Error::ConnectionNotFound { id });
    };

    if let Some(entry) = conns.get(&id) {
        let _ = entry.terminate_tx.send(true);
        Ok(())
    } else {
        Err(Error::ConnectionNotFound { id })
    }
}

/// Async I/O stream wrapper that counts bytes read and written.
pub struct CountingStream<S> {
    pub inner: S,
    pub bytes_read: Arc<AtomicU64>,
    pub bytes_written: Arc<AtomicU64>,
}

impl<S: AsyncRead + Unpin> AsyncRead for CountingStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        let after = buf.filled().len();
        if after > before {
            self.bytes_read
                .fetch_add((after - before) as u64, Ordering::Relaxed);
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res {
            self.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Copies data bidirectionally while updating byte counters and listening for termination signals.
pub async fn copy_tracked_bidirectional<A, B>(
    a: A,
    mut b: B,
    bytes_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    mut cancel_rx: watch::Receiver<bool>,
) -> IoResult<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let mut tracked_a = CountingStream {
        inner: a,
        bytes_read: bytes_received,
        bytes_written: bytes_sent,
    };

    tokio::select! {
        res = tokio::io::copy_bidirectional(&mut tracked_a, &mut b) => {
            res.map(|_| ())
        }
        _ = async {
            loop {
                if cancel_rx.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
                if *cancel_rx.borrow() {
                    break;
                }
            }
        } => {
            let _ = tracked_a.inner.shutdown().await;
            let _ = b.shutdown().await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tracker_lifecycle_and_termination() {
        let proxy_id = ShortId::new();
        let (id, guard, cancel_rx, sent, recv) = register(
            Some(proxy_id),
            "web-proxy",
            ConnectionProtocol::WebSocket,
            "127.0.0.1:12345",
            "10.0.0.1:8080",
        );

        let list_all = list(None);
        assert!(list_all.iter().any(|c| c.id == id));
        assert_eq!(
            list_all.iter().find(|c| c.id == id).unwrap().proxy_name,
            "web-proxy"
        );

        sent.fetch_add(1024, Ordering::Relaxed);
        recv.fetch_add(512, Ordering::Relaxed);

        let list_filtered = list(Some(&proxy_id));
        assert_eq!(list_filtered.len(), 1);
        assert_eq!(list_filtered[0].bytes_sent, 1024);
        assert_eq!(list_filtered[0].bytes_received, 512);

        // Terminate
        assert!(!*cancel_rx.borrow());
        terminate(id).unwrap();
        assert!(*cancel_rx.borrow());

        // Drop guard unregisters
        drop(guard);
        assert!(list(None).iter().all(|c| c.id != id));

        // Terminating non-existent returns error
        assert!(terminate(id).is_err());
    }
}
