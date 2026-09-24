//! The agent port of the master. A connection with a client certificate of the agent CA becomes
//! the session of its target. A connection without one may send one enrollment frame.

use super::frame::{read_frame, write_frame};
use super::link::Link;
use super::pki::fingerprint;
use super::protocol::{AgentOutput, AgentRequest, EnrollRequest, EnrollResponse, Enrolled};
use super::registry::AgentRegistry;
use crate::clock::unix_ms;
use anyhow::{anyhow, bail};
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{info, warn};

/// The targets and the enrollment of the agents.
#[async_trait::async_trait]
pub trait AgentDirectory: Send + Sync {
    /// The target of the agent certificate with the SHA-256 `fingerprint`.
    async fn target_of(&self, fingerprint: &str) -> anyhow::Result<Option<ShortId>>;

    /// Checks the token secret, signs the certificate and stores its fingerprint.
    async fn enroll(&self, request: &EnrollRequest) -> anyhow::Result<Enrolled>;

    /// Records the time of the last contact with the agent of `target`.
    async fn seen(&self, target: ShortId, now: u64) -> anyhow::Result<()>;
}

/// The durations of the link.
#[derive(Debug, Clone, Copy)]
pub struct LinkTiming {
    /// The longest TLS handshake and enrollment frame.
    pub handshake: Duration,
    /// The pause between two pings of a connected agent.
    pub ping_interval: Duration,
    /// The longest wait for a pong. An agent that does not answer is offline.
    pub ping_timeout: Duration,
}

impl Default for LinkTiming {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(10),
            ping_interval: Duration::from_secs(10),
            ping_timeout: Duration::from_secs(20),
        }
    }
}

pub struct AgentListener {
    acceptor: TlsAcceptor,
    directory: Arc<dyn AgentDirectory>,
    registry: Arc<AgentRegistry>,
    timing: LinkTiming,
}

impl AgentListener {
    pub fn new(
        acceptor: TlsAcceptor,
        directory: Arc<dyn AgentDirectory>,
        registry: Arc<AgentRegistry>,
        timing: LinkTiming,
    ) -> Arc<Self> {
        Arc::new(Self {
            acceptor,
            directory,
            registry,
            timing,
        })
    }

    /// Accepts agents on `listener` until the server shuts down.
    pub async fn run(
        self: Arc<Self>,
        listener: TcpListener,
        mut events: broadcast::Receiver<ServerEvent>,
    ) {
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        tokio::spawn(self.clone().connection(stream, peer));
                    }
                    Err(err) => warn!(%err, "failed to accept an agent connection"),
                },
                event = events.recv() => match event {
                    Ok(ServerEvent::Shutdown) | Err(RecvError::Closed) => break,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                },
            }
        }
    }

    async fn connection(self: Arc<Self>, stream: TcpStream, peer: SocketAddr) {
        if let Err(err) = self.serve(stream).await {
            warn!(%peer, err = format!("{err:#}"), "an agent connection failed");
        }
    }

    async fn serve(&self, stream: TcpStream) -> anyhow::Result<()> {
        let tls = timeout(self.timing.handshake, self.acceptor.accept(stream)).await??;
        let certificate = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .map(|der| fingerprint(der));
        match certificate {
            Some(certificate) => self.session(tls, &certificate).await,
            None => self.enroll(tls).await,
        }
    }

    async fn enroll(&self, mut tls: TlsStream<TcpStream>) -> anyhow::Result<()> {
        let request: EnrollRequest = timeout(self.timing.handshake, read_frame(&mut tls)).await??;
        let response = match self.directory.enroll(&request).await {
            Ok(enrolled) => {
                info!(target = %enrolled.target, version = request.version, "an agent enrolled");
                EnrollResponse::Enrolled(enrolled)
            }
            Err(err) => {
                warn!(err = format!("{err:#}"), "an agent enrollment was refused");
                EnrollResponse::Refused {
                    message: refusal(&err),
                }
            }
        };
        write_frame(&mut tls, &response).await?;
        tls.shutdown().await?;
        Ok(())
    }

    async fn session(&self, tls: TlsStream<TcpStream>, certificate: &str) -> anyhow::Result<()> {
        let Some(target) = self.directory.target_of(certificate).await? else {
            bail!("the agent certificate belongs to no target");
        };
        let (link, task) = Link::spawn(tls.compat());
        let version = match self.ping(&link).await {
            Ok(version) => version,
            Err(err) => {
                task.abort();
                return Err(err);
            }
        };
        info!(%target, version, "an agent connected");
        let now = unix_ms();
        let generation =
            self.registry
                .insert(target, link.clone(), version, task.abort_handle(), now);
        self.record_contact(target, now).await;
        self.heartbeat(target, generation, &link).await;
        self.registry.remove(target, generation);
        self.record_contact(target, unix_ms()).await;
        info!(%target, "an agent disconnected");
        Ok(())
    }

    /// Pings the agent until it stops answering or its session ends.
    async fn heartbeat(&self, target: ShortId, generation: u64, link: &Link) {
        loop {
            tokio::time::sleep(self.timing.ping_interval).await;
            if self.ping(link).await.is_err() {
                return;
            }
            self.registry.seen(target, generation, unix_ms());
        }
    }

    /// Pings the agent and returns its version.
    async fn ping(&self, link: &Link) -> anyhow::Result<String> {
        let output = timeout(self.timing.ping_timeout, link.request(&AgentRequest::Ping))
            .await
            .map_err(|_| anyhow!("the agent did not answer a ping"))??;
        match output {
            AgentOutput::Pong { version } => Ok(version),
        }
    }

    async fn record_contact(&self, target: ShortId, now: u64) {
        if let Err(err) = self.directory.seen(target, now).await {
            warn!(%target, err = format!("{err:#}"), "failed to record the contact of an agent");
        }
    }
}

/// The reason of a refused enrollment for the agent. Only an API error travels; any other failure
/// can hold file paths of the master.
fn refusal(err: &anyhow::Error) -> String {
    match err.downcast_ref::<r3v3rs3_api::error::Error>() {
        Some(err) => err.to_string(),
        None => "the enrollment failed, see the log of the master".to_string(),
    }
}
