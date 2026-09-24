//! The agent side of the link: `r3v3rs3 agent`. The agent enrolls once with its token, keeps its
//! key and certificate in its data directory, and then holds an mTLS connection to the master,
//! which it opens again after every failure.

use super::executor::Executor;
use super::frame::{read_frame, write_frame};
use super::link::serve;
use super::pki::{
    agent_client_config, enrollment_client_config, master_server_name, new_agent_key,
};
use super::protocol::{EnrollRequest, EnrollResponse, Enrolled};
use super::token::EnrollmentToken;
use crate::clock::unix_ms;
use crate::config::file::write_private;
use anyhow::{Context, anyhow, bail};
use r3v3rs3_api::id::ShortId;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::fs;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::ClientConfig;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{info, warn};

const KEY_FILE: &str = "agent.key";
const CERTIFICATE_FILE: &str = "agent.pem";
const CA_FILE: &str = "ca.pem";
const TARGET_FILE: &str = "target";

/// The version that the agent reports to the master.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The durations of the agent side of the link.
#[derive(Debug, Clone, Copy)]
pub struct AgentTiming {
    /// The longest TCP connect and TLS handshake.
    pub connect: Duration,
    /// A connection without a request of the master for this long is dead. The master pings
    /// every 10 seconds.
    pub idle: Duration,
    /// The first pause before a new connection. Every failure doubles it up to `max_backoff`.
    pub backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for AgentTiming {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            idle: Duration::from_secs(45),
            backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
        }
    }
}

/// The key, the certificate and the CA of an enrolled agent.
#[derive(Clone)]
pub struct AgentIdentity {
    pub target: ShortId,
    certificate: String,
    key: String,
    ca: String,
}

impl AgentIdentity {
    /// The identity in `dir`, or `None` before the enrollment.
    pub async fn load(dir: &Path) -> anyhow::Result<Option<Self>> {
        if !fs::try_exists(dir.join(TARGET_FILE)).await? {
            return Ok(None);
        }
        let target = read_text(&dir.join(TARGET_FILE)).await?;
        Ok(Some(Self {
            target: target.trim().parse()?,
            certificate: read_text(&dir.join(CERTIFICATE_FILE)).await?,
            key: read_text(&dir.join(KEY_FILE)).await?,
            ca: read_text(&dir.join(CA_FILE)).await?,
        }))
    }

    /// Writes the identity to `dir`. The target file comes last, because it marks a complete
    /// identity.
    async fn save(&self, dir: &Path) -> anyhow::Result<()> {
        create_private_dir(dir).await?;
        write_private(&dir.join(KEY_FILE), self.key.clone()).await?;
        fs::write(dir.join(CERTIFICATE_FILE), &self.certificate).await?;
        fs::write(dir.join(CA_FILE), &self.ca).await?;
        fs::write(dir.join(TARGET_FILE), self.target.to_string()).await?;
        Ok(())
    }

    fn client_config(&self) -> anyhow::Result<ClientConfig> {
        agent_client_config(&self.ca, &self.certificate, &self.key)
    }
}

async fn read_text(path: &Path) -> anyhow::Result<String> {
    fs::read_to_string(path)
        .await
        .with_context(|| format!("cannot read {}", path.display()))
}

async fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(dir)
        .await
        .with_context(|| format!("cannot create {}", dir.display()))?;
    fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}

pub struct AgentClient {
    /// The agent port of the master as `host:port`.
    master: String,
    data_dir: PathBuf,
    timing: AgentTiming,
    executor: Arc<Executor>,
}

impl AgentClient {
    /// A client that runs the requests of the master with `executor`.
    pub fn new(master: String, data_dir: PathBuf, timing: AgentTiming, executor: Executor) -> Self {
        Self {
            master,
            data_dir,
            timing,
            executor: Arc::new(executor),
        }
    }

    /// The stored identity, or a new one from `token`. An agent with an identity ignores the
    /// token.
    pub async fn identity(&self, token: Option<&EnrollmentToken>) -> anyhow::Result<AgentIdentity> {
        if let Some(identity) = AgentIdentity::load(&self.data_dir).await? {
            return Ok(identity);
        }
        let token = token.context("the agent is not enrolled yet, so it needs --token")?;
        let identity = self.enroll(token).await?;
        identity.save(&self.data_dir).await?;
        info!(target = %identity.target, "the agent enrolled");
        Ok(identity)
    }

    /// Sends the enrollment frame and returns the new identity.
    async fn enroll(&self, token: &EnrollmentToken) -> anyhow::Result<AgentIdentity> {
        let (key, csr) = new_agent_key()?;
        let config = enrollment_client_config(&token.ca_hash);
        let mut tls = self.connect(config).await?;
        let request = EnrollRequest {
            secret: token.secret.clone(),
            csr,
            version: VERSION.to_string(),
        };
        write_frame(&mut tls, &request).await?;
        match timeout(self.timing.connect, read_frame(&mut tls)).await?? {
            EnrollResponse::Enrolled(Enrolled {
                target,
                certificate,
                ca,
            }) => Ok(AgentIdentity {
                target,
                certificate,
                key,
                ca,
            }),
            EnrollResponse::Refused { message } => {
                bail!("the master refused the enrollment: {message}")
            }
        }
    }

    async fn connect(&self, config: ClientConfig) -> anyhow::Result<TlsStream<TcpStream>> {
        let connect = async {
            let tcp = TcpStream::connect(&self.master).await?;
            let connector = TlsConnector::from(Arc::new(config));
            anyhow::Ok(connector.connect(master_server_name()?, tcp).await?)
        };
        timeout(self.timing.connect, connect)
            .await
            .map_err(|_| anyhow!("the connection to {} timed out", self.master))?
            .with_context(|| format!("cannot connect to {}", self.master))
    }

    /// Holds the connection to the master and opens it again after every failure.
    pub async fn run(&self, identity: &AgentIdentity) -> anyhow::Result<()> {
        let config = identity.client_config()?;
        let mut backoff = self.timing.backoff;
        loop {
            let started = tokio::time::Instant::now();
            match self.session(config.clone()).await {
                Ok(()) => info!("the master closed the connection"),
                Err(err) => warn!(
                    err = format!("{err:#}"),
                    "the connection to the master failed"
                ),
            }
            // A connection that lasted resets the backoff.
            if started.elapsed() > self.timing.max_backoff {
                backoff = self.timing.backoff;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(self.timing.max_backoff);
        }
    }

    /// One connection: serves the streams of the master until the connection closes or stays
    /// idle.
    async fn session(&self, config: ClientConfig) -> anyhow::Result<()> {
        let tls = self.connect(config).await?;
        info!(master = self.master, "connected to the master");
        let last_request = Arc::new(AtomicU64::new(unix_ms()));
        let seen = last_request.clone();
        let executor = self.executor.clone();
        let served = serve(tls.compat(), move |stream| {
            seen.store(unix_ms(), Ordering::Relaxed);
            let executor = executor.clone();
            tokio::spawn(async move {
                if let Err(err) = executor.answer(stream).await {
                    warn!(
                        err = format!("{err:#}"),
                        "failed to answer a request of the master"
                    );
                }
            });
        });
        tokio::select! {
            result = served => Ok(result?),
            () = idle(&last_request, self.timing.idle) => {
                bail!("the master sent no request for {} seconds", self.timing.idle.as_secs())
            }
        }
    }
}

/// Returns when no request arrived for `limit`.
async fn idle(last_request: &AtomicU64, limit: Duration) {
    let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
    loop {
        tokio::time::sleep(limit / 4).await;
        let quiet = unix_ms().saturating_sub(last_request.load(Ordering::Relaxed));
        if quiet > limit_ms {
            return;
        }
    }
}
