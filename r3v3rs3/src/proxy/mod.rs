use self::{
    http::{HttpPortContext, HttpStarter, SessionService},
    tcp::{TcpPortContext, TcpStarter},
    udp::UdpPortContext,
};
use crate::server::cert_list::CertList;
use once_cell::sync::OnceCell;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::multiaddr::Multiaddr;
use r3v3rs3_api::port::{PortStatus, SocketState};
use r3v3rs3_api::{
    port::{Port, PortEntry},
    proxy::ProxyEntry,
};
use std::future::Future;
use tokio::{io::BufStream, net::TcpStream};
use tracing::{error, Instrument, Span};

pub mod health;
pub mod http;
pub mod proxy_protocol;
pub mod tcp;
pub mod tls;
pub mod udp;

/// Runs the task of a client connection in the span and logs its error.
fn spawn_connection<F>(span: Span, task: F)
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    tokio::spawn(
        async move {
            if let Err(err) = task.await {
                error!("{err}");
            }
        }
        .instrument(span),
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortContextEvent {
    SocketStateUpdated(SocketState),
}

#[derive(Debug)]
pub struct PortContext {
    pub entry: PortEntry,
    pub kind: PortContextKind,
}

impl PortContext {
    pub fn new(entry: PortEntry) -> Result<Self, Error> {
        if let Some(proxy_protocol) = &entry.port.opts.proxy_protocol {
            proxy_protocol.validate(&entry.port.listen)?;
        }
        let kind = if entry.port.listen.is_quic() {
            PortContextKind::Http3(HttpPortContext::new(&entry)?)
        } else if entry.port.listen.is_udp() {
            PortContextKind::Udp(UdpPortContext::new(&entry)?)
        } else if entry.port.listen.is_http() {
            PortContextKind::Http(HttpPortContext::new(&entry)?)
        } else {
            PortContextKind::Tcp(TcpPortContext::new(&entry)?)
        };
        Ok(Self { entry, kind })
    }

    pub fn reserved() -> Self {
        Self {
            entry: PortEntry {
                id: "reserved".parse().unwrap(),
                port: Port {
                    active: true,
                    name: String::new(),
                    listen: Multiaddr::default(),
                    opts: Default::default(),
                },
            },
            kind: PortContextKind::Reserved,
        }
    }

    pub fn entry(&self) -> &PortEntry {
        &self.entry
    }

    pub fn kind(&self) -> &PortContextKind {
        &self.kind
    }

    pub fn kind_mut(&mut self) -> &mut PortContextKind {
        &mut self.kind
    }

    /// Starts the client connections of a TCP or HTTP port. `None` for another port.
    pub fn starter(&self) -> Option<ConnectionStarter> {
        match &self.kind {
            PortContextKind::Tcp(ctx) => Some(ConnectionStarter::Tcp(Box::new(ctx.starter()))),
            PortContextKind::Http(ctx) => Some(ConnectionStarter::Http(ctx.starter())),
            _ => None,
        }
    }

    pub async fn setup(
        &mut self,
        ports: &[PortEntry],
        certs: &CertList,
        proxies: Vec<ProxyEntry>,
        sessions: &std::sync::Arc<SessionService>,
    ) -> Result<(), Error> {
        match &mut self.kind {
            PortContextKind::Tcp(ctx) => ctx.setup(certs, proxies).await,
            PortContextKind::Http(ctx) => ctx.setup(ports, certs, proxies, sessions).await,
            PortContextKind::Udp(ctx) => ctx.setup(proxies).await,
            PortContextKind::Http3(ctx) => ctx.setup(ports, certs, proxies, sessions).await,
            PortContextKind::Reserved => Ok(()),
        }
    }

    pub fn apply(&mut self, new: Self) {
        match (&mut self.kind, new.kind) {
            (PortContextKind::Tcp(old), PortContextKind::Tcp(new)) => old.apply(new),
            (PortContextKind::Udp(old), PortContextKind::Udp(new)) => old.apply(new),
            (PortContextKind::Http(old), PortContextKind::Http(new)) => old.apply(new),
            (PortContextKind::Http3(old), PortContextKind::Http3(new)) => old.apply(new),
            (old, new) => *old = new,
        }
        self.entry = new.entry;
    }

    pub fn event(&mut self, event: PortContextEvent) {
        match &mut self.kind {
            PortContextKind::Tcp(ctx) => ctx.event(event),
            PortContextKind::Udp(ctx) => ctx.event(event),
            PortContextKind::Http(ctx) => ctx.event(event),
            PortContextKind::Http3(ctx) => ctx.event(event),
            PortContextKind::Reserved => (),
        }
    }

    pub fn status(&self) -> &PortStatus {
        match &self.kind {
            PortContextKind::Tcp(ctx) => ctx.status(),
            PortContextKind::Udp(ctx) => ctx.status(),
            PortContextKind::Http(ctx) => ctx.status(),
            PortContextKind::Http3(ctx) => ctx.status(),
            PortContextKind::Reserved => {
                static STATUS: OnceCell<PortStatus> = OnceCell::new();
                STATUS.get_or_init(PortStatus::default)
            }
        }
    }

    pub fn reset(&mut self) {
        match &mut self.kind {
            PortContextKind::Tcp(ctx) => ctx.reset(),
            PortContextKind::Udp(ctx) => ctx.reset(),
            PortContextKind::Http(ctx) => ctx.reset(),
            PortContextKind::Http3(ctx) => ctx.reset(),
            PortContextKind::Reserved => (),
        }
    }
}

/// Starts a client connection of a TCP or HTTP port. It holds a copy of the port settings, so the
/// connection can start in another task.
pub enum ConnectionStarter {
    /// Boxed, because the DNS resolver makes this variant much larger than the other.
    Tcp(Box<TcpStarter>),
    Http(HttpStarter),
}

impl ConnectionStarter {
    pub fn start(self, stream: BufStream<TcpStream>) {
        match self {
            Self::Tcp(starter) => starter.start(stream),
            Self::Http(starter) => starter.start(stream),
        }
    }
}

#[derive(Debug)]
pub enum PortContextKind {
    Tcp(TcpPortContext),
    Udp(UdpPortContext),
    Http(HttpPortContext),
    Http3(HttpPortContext),
    Reserved,
}
