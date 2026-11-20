use crate::fdmanifest::{ListenerFd, ListenerProto};
use crate::proxy::transparent;
use crate::proxy::udp::UdpDatagram;
use crate::proxy::{PortContext, PortContextEvent, PortContextKind};
use futures::{Stream, StreamExt};
use r3v3rs3_api::port::SocketState;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::net::UdpSocket;
use tracing::{Level, error, info, span};

#[derive(Debug)]
pub struct UdpListenerPool {
    listeners: Vec<UdpListenerStream>,
}

impl UdpListenerPool {
    pub fn new() -> Self {
        Self {
            listeners: Vec::new(),
        }
    }

    pub fn has_active_listeners(&self) -> bool {
        !self.listeners.is_empty()
    }

    /// Adds a socket that another process opened. The next `update` keeps it, because its
    /// address is among the addresses that the ports use.
    pub fn adopt(&mut self, socket: UdpSocket) {
        self.listeners.push(UdpListenerStream {
            index: self.listeners.len(),
            config_index: 0,
            inner: Arc::new(socket),
            orig_dst: false,
        });
    }

    /// Duplicates of the file descriptors of the listening sockets.
    pub fn listener_fds(&self) -> io::Result<Vec<ListenerFd>> {
        use std::os::fd::{AsRawFd, BorrowedFd};
        self.listeners
            .iter()
            .map(|listener| {
                let addr = listener.inner.local_addr()?;
                let fd = unsafe { BorrowedFd::borrow_raw(listener.inner.as_ref().as_raw_fd()) }
                    .try_clone_to_owned()?;
                Ok(ListenerFd::new(ListenerProto::Udp, addr, fd))
            })
            .collect()
    }

    pub async fn remove_unused_ports(&mut self, ports: &[PortContext]) {
        let used_addrs = ports
            .iter()
            .filter_map(|ctx| match ctx.kind() {
                PortContextKind::Udp(state) => Some(state.listen),
                _ => None,
            })
            .collect::<HashSet<_>>();

        self.listeners.retain(|listener| {
            if let Ok(addr) = listener.inner.local_addr() {
                used_addrs.contains(&addr)
            } else {
                false
            }
        });
    }

    pub async fn update(&mut self, ports: &mut [PortContext]) {
        let used_addrs = ports
            .iter()
            .filter_map(|ctx| match ctx.kind() {
                PortContextKind::Udp(state) => Some(state.listen),
                _ => None,
            })
            .collect::<HashSet<_>>();

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

        for (index, ctx) in ports.iter_mut().enumerate() {
            let span = span!(Level::INFO, "port", resource_id = ctx.entry.id.to_string());
            let (bind, range) = match ctx.kind() {
                PortContextKind::Udp(state) => (state.listen, state.ports.port_count() > 1),
                _ => continue,
            };
            let (listener, state) = if !ctx.entry.port.active {
                (None, SocketState::Inactive)
            } else if let Some(listener) = listeners.remove(&bind) {
                (Some(listener), SocketState::Listening)
            } else {
                span.in_scope(|| {
                    info!(%bind, range, "listening on udp port");
                });
                match create_udp_socket(bind, range) {
                    Ok(sock) => (
                        Some(UdpListenerStream {
                            index: 0,
                            config_index: 0,
                            inner: Arc::new(sock),
                            orig_dst: range,
                        }),
                        SocketState::Listening,
                    ),
                    Err(err) => {
                        let _enter = span.enter();
                        error!(%bind, %err, "failed to listen on udp port");
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
                sock.index = self.listeners.len();
                sock.config_index = index;
                // An adopted socket keeps its options, but not this flag.
                sock.orig_dst = range;
                self.listeners.push(sock);
            }
            ctx.event(PortContextEvent::SocketStateUpdated(state));
        }
    }

    pub async fn select(&mut self) -> Option<(usize, usize, UdpDatagram)> {
        let streams = &mut self.listeners;
        match futures::stream::select_all(streams.iter_mut()).next().await {
            Some((index, config_index, Ok(datagram))) => Some((index, config_index, datagram)),
            _ => None,
        }
    }

    pub fn socket(&self, index: usize) -> Option<Arc<UdpSocket>> {
        self.listeners
            .get(index)
            .map(|listener| listener.inner.clone())
    }
}

/// A UDP listening socket. A range socket is transparent and reads the original destination of
/// every datagram, because TPROXY sends the whole range to it (needs `CAP_NET_ADMIN`).
fn create_udp_socket(addr: SocketAddr, range: bool) -> io::Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(addr),
        socket2::Type::DGRAM,
        None,
    )?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    if range {
        transparent::set_transparent(&socket, addr.is_ipv6())?;
        transparent::set_recv_orig_dst(&socket, addr.is_ipv6())?;
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}

/// The largest UDP payload over IPv4.
const MAX_DATAGRAM: usize = 65527;

#[derive(Debug)]
struct UdpListenerStream {
    index: usize,
    config_index: usize,
    inner: Arc<UdpSocket>,
    /// Whether each datagram carries its original destination (a range port).
    orig_dst: bool,
}

impl UdpListenerStream {
    fn poll_plain(&self, cx: &mut Context<'_>) -> Poll<io::Result<UdpDatagram>> {
        let mut buf = vec![0; MAX_DATAGRAM];
        let mut read_buf = tokio::io::ReadBuf::new(&mut buf);
        self.inner
            .poll_recv_from(cx, &mut read_buf)
            .map_ok(|client| {
                let data = read_buf.filled().to_vec();
                UdpDatagram {
                    client,
                    local: None,
                    data,
                }
            })
    }

    fn poll_with_orig_dst(&self, cx: &mut Context<'_>) -> Poll<io::Result<UdpDatagram>> {
        loop {
            futures::ready!(self.inner.poll_recv_ready(cx))?;
            let mut buf = vec![0; MAX_DATAGRAM];
            let received = self.inner.try_io(tokio::io::Interest::READABLE, || {
                transparent::recv_with_orig_dst(socket2::SockRef::from(&*self.inner), &mut buf)
            });
            match received {
                Ok((size, client, local)) => {
                    buf.truncate(size);
                    return Poll::Ready(Ok(UdpDatagram {
                        client,
                        local,
                        data: buf,
                    }));
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                Err(err) => return Poll::Ready(Err(err)),
            }
        }
    }
}

impl Stream for UdpListenerStream {
    type Item = (usize, usize, io::Result<UdpDatagram>);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let received = match self.orig_dst {
            true => self.poll_with_orig_dst(cx),
            false => self.poll_plain(cx),
        };
        received.map(|result| Some((self.index, self.config_index, result)))
    }
}
