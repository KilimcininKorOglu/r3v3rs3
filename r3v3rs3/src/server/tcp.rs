use crate::proxy::{PortContext, PortContextEvent, PortContextKind};
use futures::{Stream, StreamExt};
use r3v3rs3_api::port::SocketState;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::net::{TcpListener, TcpStream};
use tracing::{Level, error, info, span};

const SOCKET_BACKLOG_SIZE: i32 = 128;

#[derive(Debug)]
pub struct TcpListenerPool {
    listeners: Vec<TcpListenerStream>,
    /// The listening addresses of the active ACME challenges.
    challenge_addrs: Vec<SocketAddr>,
}

impl TcpListenerPool {
    pub fn new() -> Self {
        Self {
            listeners: Vec::new(),
            challenge_addrs: Vec::new(),
        }
    }

    pub fn set_challenge_addrs(&mut self, addrs: Vec<SocketAddr>) {
        self.challenge_addrs = addrs;
    }

    pub fn has_active_listeners(&self) -> bool {
        !self.listeners.is_empty()
    }

    pub async fn remove_unused_ports(&mut self, ports: &[PortContext]) {
        let used_addrs = ports.iter().filter_map(listen_addr).collect::<HashSet<_>>();

        self.listeners.retain(|listener| {
            if let Ok(addr) = listener.inner.local_addr() {
                used_addrs.contains(&addr)
            } else {
                false
            }
        });
    }

    pub async fn update(&mut self, ports: &mut [PortContext]) {
        // A challenge address needs its own listener when no port uses its port.
        let reserved_addrs = self
            .challenge_addrs
            .iter()
            .copied()
            .filter(|addr| {
                !ports
                    .iter()
                    .filter_map(listen_addr)
                    .any(|listen| listen.port() == addr.port())
            })
            .collect::<Vec<_>>();
        let mut reserved_ports = reserved_addrs
            .iter()
            .map(|_| PortContext::reserved())
            .collect::<Vec<_>>();
        // The bind address of each port, then of each reserved port, in the order of the loop.
        let bind_addrs = ports
            .iter()
            .map(listen_addr)
            .chain(reserved_addrs.into_iter().map(Some))
            .collect::<Vec<_>>();
        let used_addrs = bind_addrs.iter().flatten().copied().collect::<HashSet<_>>();

        let mut listeners: HashMap<_, _> = self
            .listeners
            .drain(..)
            .filter_map(|listener| {
                listener
                    .inner
                    .local_addr()
                    .ok()
                    .map(|addr| (addr, listener))
            })
            .filter(|(addr, _)| used_addrs.contains(addr))
            .collect();

        for (index, ctx) in ports
            .iter_mut()
            .chain(reserved_ports.iter_mut())
            .enumerate()
        {
            let span = span!(Level::INFO, "port", resource_id = ctx.entry.id.to_string());
            let Some(bind) = bind_addrs.get(index).copied().flatten() else {
                continue;
            };
            let (listener, state) = if !ctx.entry.port.active {
                (None, SocketState::Inactive)
            } else if let Some(listener) = listeners.remove(&bind) {
                (Some(listener), SocketState::Listening)
            } else {
                span.in_scope(|| {
                    info!(%bind, "listening on tcp port");
                });
                match create_tcp_listener(bind) {
                    Ok(sock) => (
                        Some(TcpListenerStream {
                            index: 0,
                            inner: sock,
                        }),
                        SocketState::Listening,
                    ),
                    Err(err) => {
                        let _enter = span.enter();
                        error!(%bind, %err, "failed to listen on tcp port");
                        let error = match err.kind() {
                            io::ErrorKind::AddrInUse => SocketState::AddressAlreadyInUse,
                            io::ErrorKind::PermissionDenied => SocketState::PermissionDenied,
                            io::ErrorKind::AddrNotAvailable => SocketState::AddressNotAvailable,
                            _ => SocketState::Error,
                        };
                        (None, error)
                    }
                }
            };
            if let Some(mut sock) = listener {
                sock.index = index;
                self.listeners.push(sock);
            }
            ctx.event(PortContextEvent::SocketStateUpdated(state));
        }
    }

    pub async fn select(&mut self) -> Option<(usize, TcpStream)> {
        let streams = &mut self.listeners;
        match futures::stream::select_all(streams).next().await {
            Some((index, Ok(sock))) => Some((index, sock)),
            _ => None,
        }
    }
}

/// The listening address of a TCP or HTTP port. `None` for another port.
fn listen_addr(ctx: &PortContext) -> Option<SocketAddr> {
    match ctx.kind() {
        PortContextKind::Tcp(state) => Some(state.listen),
        PortContextKind::Http(state) => Some(state.listen),
        _ => None,
    }
}

fn create_tcp_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(addr),
        socket2::Type::STREAM,
        None,
    )?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(SOCKET_BACKLOG_SIZE)?;
    TcpListener::from_std(socket.into())
}

#[derive(Debug)]
struct TcpListenerStream {
    index: usize,
    inner: TcpListener,
}

impl Stream for TcpListenerStream {
    type Item = (usize, io::Result<TcpStream>);

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<(usize, io::Result<TcpStream>)>> {
        match self.inner.poll_accept(cx) {
            Poll::Ready(Ok((stream, _))) => Poll::Ready(Some((self.index, Ok(stream)))),
            Poll::Ready(Err(err)) => Poll::Ready(Some((self.index, Err(err)))),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::port::{Port, PortEntry};

    #[tokio::test]
    async fn only_the_reserved_port_listens_on_the_challenge_address() {
        let entry = PortEntry {
            id: "udp".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: "/ip4/127.0.0.1/udp/5353".parse().unwrap(),
                opts: Default::default(),
            },
        };
        let mut ports = [PortContext::new(entry).unwrap()];
        let mut pool = TcpListenerPool::new();
        // The HTTP-01 and the TLS-ALPN-01 challenges each get a listener.
        let addr = "127.0.0.1:0".parse().unwrap();
        pool.set_challenge_addrs(vec![addr, addr]);
        pool.update(&mut ports).await;

        assert_eq!(ports[0].status().state.socket, SocketState::Unknown);
        assert_eq!(pool.listeners.len(), 2);

        pool.set_challenge_addrs(Vec::new());
        pool.update(&mut ports).await;
        assert!(pool.listeners.is_empty());
    }
}
