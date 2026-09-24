//! The link between a master and an agent over a loopback TCP connection.

use super::client::{AgentClient, AgentIdentity, AgentTiming, VERSION};
use super::listener::{AgentDirectory, AgentListener, LinkTiming};
use super::pki::{AgentPki, fingerprint, pem_certificates};
use super::protocol::{EnrollRequest, Enrolled};
use super::registry::AgentRegistry;
use super::token::{EnrollmentToken, secret_hash};
use crate::platform::fake::FakeRuntime;
use anyhow::{Context, anyhow};
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_rustls::TlsAcceptor;

/// The targets of a test in memory.
struct Directory {
    pki: AgentPki,
    /// Token hash to target.
    tokens: Mutex<HashMap<String, ShortId>>,
    /// Certificate fingerprint to target.
    certificates: Mutex<HashMap<String, ShortId>>,
    contacts: Mutex<Vec<ShortId>>,
}

#[async_trait::async_trait]
impl AgentDirectory for Directory {
    async fn target_of(&self, fingerprint: &str) -> anyhow::Result<Option<ShortId>> {
        Ok(self.certificates.lock().unwrap().get(fingerprint).copied())
    }

    async fn enroll(&self, request: &EnrollRequest, _: IpAddr) -> anyhow::Result<Enrolled> {
        let target = self
            .tokens
            .lock()
            .unwrap()
            .remove(&secret_hash(&request.secret))
            .context("unknown token")?;
        let certificate = self.pki.sign(&request.csr, target)?;
        let der = pem_certificates(&certificate)?;
        self.certificates
            .lock()
            .unwrap()
            .insert(fingerprint(&der[0]), target);
        Ok(Enrolled {
            target,
            certificate,
            ca: self.pki.ca_pem()?,
        })
    }

    async fn seen(&self, target: ShortId, _: u64) -> anyhow::Result<()> {
        self.contacts.lock().unwrap().push(target);
        Ok(())
    }
}

struct Master {
    directory: Arc<Directory>,
    registry: Arc<AgentRegistry>,
    address: String,
    events: broadcast::Sender<ServerEvent>,
    dir: PathBuf,
}

impl Master {
    async fn start() -> anyhow::Result<Self> {
        let dir = temp_dir("master");
        let pki = AgentPki::load_or_create(&dir).await?;
        let acceptor = TlsAcceptor::from(Arc::new(pki.server_config()?));
        let directory = Arc::new(Directory {
            pki,
            tokens: Mutex::default(),
            certificates: Mutex::default(),
            contacts: Mutex::default(),
        });
        let registry = Arc::new(AgentRegistry::default());
        let timing = LinkTiming {
            handshake: Duration::from_secs(5),
            ping_interval: Duration::from_millis(50),
            ping_timeout: Duration::from_millis(500),
        };
        let listener = AgentListener::new(acceptor, directory.clone(), registry.clone(), timing);
        let socket = TcpListener::bind("127.0.0.1:0").await?;
        let address = socket.local_addr()?.to_string();
        let (events, receiver) = broadcast::channel(4);
        tokio::spawn(listener.run(socket, receiver));
        Ok(Self {
            directory,
            registry,
            address,
            events,
            dir,
        })
    }

    fn token(&self, target: ShortId) -> EnrollmentToken {
        let token = EnrollmentToken::new(self.directory.pki.ca_hash());
        self.directory
            .tokens
            .lock()
            .unwrap()
            .insert(token.secret_hash(), target);
        token
    }

    fn agent(&self, name: &str) -> (AgentClient, PathBuf) {
        let dir = temp_dir(name);
        let timing = AgentTiming {
            connect: Duration::from_secs(5),
            idle: Duration::from_secs(2),
            backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(200),
        };
        (
            AgentClient::new(
                self.address.clone(),
                dir.clone(),
                timing,
                Arc::new(FakeRuntime::default()),
            ),
            dir,
        )
    }

    async fn wait_online(&self, target: ShortId, online: bool) -> anyhow::Result<()> {
        for _ in 0..200 {
            if self.registry.status(target).is_some() == online {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err(anyhow!(
            "the agent of {target} did not become online={online}"
        ))
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        let _ = self.events.send(ServerEvent::Shutdown);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "r3v3rs3-agent-{name}-{}",
        hex::encode(rand::random::<[u8; 8]>())
    ))
}

fn target() -> ShortId {
    "fzn-txd".parse().unwrap()
}

#[tokio::test]
async fn an_agent_enrolls_then_connects_with_its_certificate() -> anyhow::Result<()> {
    let master = Master::start().await?;
    let (agent, dir) = master.agent("enroll");
    let identity = agent.identity(Some(&master.token(target()))).await?;
    assert_eq!(identity.target, target());

    let key_mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(dir.join("agent.key"))?.permissions(),
    );
    assert_eq!(key_mode & 0o777, 0o600);
    // A second start reads the stored identity and needs no token.
    let stored = AgentIdentity::load(&dir).await?.context("identity")?;
    assert_eq!(stored.target, target());

    let running = tokio::spawn(async move { agent.run(&identity).await });
    master.wait_online(target(), true).await?;
    let status = master.registry.status(target()).context("status")?;
    assert_eq!(status.version, VERSION);
    assert!(
        master
            .directory
            .contacts
            .lock()
            .unwrap()
            .contains(&target())
    );

    running.abort();
    master.wait_online(target(), false).await?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[tokio::test]
async fn a_token_enrolls_only_once() -> anyhow::Result<()> {
    let master = Master::start().await?;
    let token = master.token(target());
    let (first, first_dir) = master.agent("once-a");
    first.identity(Some(&token)).await?;
    let (second, second_dir) = master.agent("once-b");
    let err = second.identity(Some(&token)).await.err().context("error")?;
    assert!(format!("{err:#}").contains("refused"), "{err:#}");
    assert!(AgentIdentity::load(&second_dir).await?.is_none());
    std::fs::remove_dir_all(first_dir)?;
    Ok(())
}

#[tokio::test]
async fn an_agent_without_an_identity_needs_a_token() -> anyhow::Result<()> {
    let master = Master::start().await?;
    let (agent, _) = master.agent("no-token");
    let err = agent.identity(None).await.err().context("error")?;
    assert!(err.to_string().contains("--token"));
    Ok(())
}

#[tokio::test]
async fn a_revoked_certificate_cannot_connect() -> anyhow::Result<()> {
    let master = Master::start().await?;
    let (agent, dir) = master.agent("revoked");
    let identity = agent.identity(Some(&master.token(target()))).await?;
    master.directory.certificates.lock().unwrap().clear();
    let running = tokio::spawn(async move { agent.run(&identity).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(master.registry.status(target()).is_none());
    running.abort();
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[tokio::test]
async fn a_token_for_another_master_is_refused_by_the_agent() -> anyhow::Result<()> {
    let master = Master::start().await?;
    let other = Master::start().await?;
    // The token names the CA of the other master, so the agent does not send the secret here.
    let token = other.token(target());
    let (agent, _) = master.agent("wrong-master");
    assert!(agent.identity(Some(&token)).await.is_err());
    assert!(other.directory.tokens.lock().unwrap().len() == 1);
    Ok(())
}

#[tokio::test]
async fn the_heartbeat_keeps_the_contact_time_current() -> anyhow::Result<()> {
    let master = Master::start().await?;
    let (agent, dir) = master.agent("heartbeat");
    let identity = agent.identity(Some(&master.token(target()))).await?;
    let running = tokio::spawn(async move { agent.run(&identity).await });
    master.wait_online(target(), true).await?;
    let first = master.registry.status(target()).context("status")?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let later = master.registry.status(target()).context("status")?;
    assert!(later.last_seen_at > first.last_seen_at);
    running.abort();
    std::fs::remove_dir_all(dir)?;
    Ok(())
}
