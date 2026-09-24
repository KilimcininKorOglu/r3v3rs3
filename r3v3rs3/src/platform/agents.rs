//! The agent targets of the platform: the enrollment of an agent and the port that the agents
//! connect to.

use super::store::Write;
use super::{Platform, new_id, not_found};
use crate::agent::listener::{AgentDirectory, AgentListener, LinkTiming};
use crate::agent::pki::{AgentPki, fingerprint, pem_certificates};
use crate::agent::protocol::{EnrollRequest, Enrolled};
use crate::agent::token::{EnrollmentToken, secret_hash};
use crate::audit::AuditRecord;
use crate::notify::NotificationEvent;
use anyhow::{Context, anyhow};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{TargetEntry, TargetKind, TargetRequest, TargetToken};
use std::net::IpAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_rustls::TlsAcceptor;
use tracing::info;

impl Platform {
    /// Listens for agents on `port` of every IPv4 address until the server shuts down.
    pub(super) async fn start_agent_listener(
        self: &Arc<Self>,
        port: u16,
        events: broadcast::Receiver<ServerEvent>,
    ) -> anyhow::Result<()> {
        let pki = self.pki.as_ref().context("the agent CA is not loaded")?;
        let acceptor = TlsAcceptor::from(Arc::new(pki.server_config()?));
        let socket = TcpListener::bind(("0.0.0.0", port))
            .await
            .with_context(|| format!("cannot listen for agents on port {port}"))?;
        info!(port, "listening for agents");
        let directory: Arc<dyn AgentDirectory> = self.clone();
        let listener = AgentListener::new(
            acceptor,
            directory,
            self.agents.clone(),
            LinkTiming::default(),
        );
        tokio::spawn(listener.run(socket, events));
        Ok(())
    }

    /// The targets with the state of their agents.
    pub async fn targets(&self) -> anyhow::Result<Vec<TargetEntry>> {
        let mut targets = self.store.targets().await?;
        for target in &mut targets {
            if let Some(status) = self.agents.status(target.id) {
                target.online = true;
                target.version = Some(status.version);
                target.last_seen_at = Some(status.last_seen_at);
            }
        }
        Ok(targets)
    }

    pub async fn target(&self, id: ShortId) -> anyhow::Result<TargetEntry> {
        self.targets()
            .await?
            .into_iter()
            .find(|target| target.id == id)
            .ok_or_else(|| not_found(id))
    }

    /// Adds an agent target and returns its first enrollment token.
    pub async fn add_target(
        &self,
        request: TargetRequest,
        now: u64,
    ) -> anyhow::Result<TargetToken> {
        let pki = self.agent_pki()?;
        let token = EnrollmentToken::new(pki.ca_hash());
        let id = self.free_target_id().await?;
        let name = request.name.as_str();
        match self
            .store
            .add_agent_target(id, name, &token.secret_hash(), now)
            .await?
        {
            Write::Done => self.target_token(id, &token).await,
            Write::NameTaken => Err(Error::TargetNameExists {
                name: name.to_string(),
            }
            .into()),
            Write::NotFound => Err(anyhow!("the target was not stored")),
        }
    }

    /// Replaces the enrollment token of an agent target. The enrolled agent keeps working until
    /// an agent enrolls with the new token.
    pub async fn new_target_token(&self, id: ShortId) -> anyhow::Result<TargetToken> {
        let pki = self.agent_pki()?;
        self.agent_target(id).await?;
        let token = EnrollmentToken::new(pki.ca_hash());
        if !self
            .store
            .set_target_token(id, &token.secret_hash())
            .await?
        {
            return Err(not_found(id));
        }
        self.target_token(id, &token).await
    }

    /// Deletes an agent target without apps and closes the connection of its agent.
    pub async fn delete_target(&self, id: ShortId) -> anyhow::Result<TargetEntry> {
        let target = self.agent_target(id).await?;
        if self.store.target_has_apps(id).await? {
            return Err(Error::TargetInUse { id }.into());
        }
        if !self.store.delete_target(id).await? {
            return Err(not_found(id));
        }
        self.agents.disconnect(id);
        self.forwarders.close_target(id);
        Ok(target)
    }

    async fn target_token(
        &self,
        id: ShortId,
        token: &EnrollmentToken,
    ) -> anyhow::Result<TargetToken> {
        Ok(TargetToken {
            target: self.target(id).await?,
            token: token.to_string(),
            agent_port: self
                .config
                .agent_port
                .context("the agent port is not set")?,
            agent_host: self.config.agent_host.clone(),
        })
    }

    /// A target that an agent runs. The local target cannot change.
    async fn agent_target(&self, id: ShortId) -> anyhow::Result<TargetEntry> {
        let target = self.target(id).await?;
        if target.kind == TargetKind::Local {
            return Err(Error::TargetReadOnly { id }.into());
        }
        Ok(target)
    }

    fn agent_pki(&self) -> Result<&AgentPki, Error> {
        self.pki.as_ref().ok_or(Error::AgentPortMissing)
    }

    /// A new id that no target uses.
    async fn free_target_id(&self) -> anyhow::Result<ShortId> {
        for _ in 0..16 {
            let id = new_id()?;
            if !self.store.has_target(id).await? {
                return Ok(id);
            }
        }
        Err(anyhow!("no free target id"))
    }

    fn record_enrollment(&self, target: &TargetEntry, version: &str, client: IpAddr) {
        if let Some(audit) = &self.audit {
            let record = AuditRecord::new(AuditAction::EnrollAgent)
                .id(target.id)
                .summary(format!("{} ({version})", target.name));
            audit.record("", Some(client), record);
        }
    }
}

/// Signs the CSR of an agent for `target`, and returns the certificate with its fingerprint.
fn sign(pki: &AgentPki, csr: &str, target: ShortId) -> anyhow::Result<(String, String)> {
    let certificate = pki.sign(csr, target)?;
    let signed = pem_certificates(&certificate)?;
    let signed = signed.first().context("the signed certificate is empty")?;
    let fingerprint = fingerprint(signed);
    Ok((certificate, fingerprint))
}

#[async_trait::async_trait]
impl AgentDirectory for Platform {
    async fn target_of(&self, fingerprint: &str) -> anyhow::Result<Option<ShortId>> {
        self.store.target_by_fingerprint(fingerprint).await
    }

    async fn enroll(&self, request: &EnrollRequest, client: IpAddr) -> anyhow::Result<Enrolled> {
        let pki = self.pki.as_ref().context("the agent CA is not loaded")?;
        let token_hash = secret_hash(&request.secret);
        let target = self
            .store
            .target_by_token(&token_hash)
            .await?
            .ok_or(Error::InvalidEnrollmentToken)?;
        let (certificate, fingerprint) = sign(pki, &request.csr, target)?;
        if !self
            .store
            .enroll_target(target, &token_hash, &fingerprint)
            .await?
        {
            return Err(Error::InvalidEnrollmentToken.into());
        }
        // The certificate of an earlier enrollment no longer matches the target.
        self.agents.disconnect(target);
        self.record_enrollment(&self.target(target).await?, &request.version, client);
        Ok(Enrolled {
            target,
            certificate,
            ca: pki.ca_pem()?,
        })
    }

    async fn seen(&self, target: ShortId, now: u64) -> anyhow::Result<()> {
        self.store.target_seen(target, now).await
    }

    async fn online(&self, target: ShortId) {
        self.notify_agent(NotificationEvent::AgentOnline, target)
            .await;
    }

    async fn offline(&self, target: ShortId) {
        self.notify_agent(NotificationEvent::AgentOffline, target)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{api_error, platform_with, request};
    use super::*;
    use crate::agent::pki::new_agent_key;
    use r3v3rs3_api::platform::{LOCAL_TARGET, PlatformConfig};
    use tokio::sync::mpsc;

    /// A platform with the agent port, so it loads the agent CA.
    async fn agent_platform() -> anyhow::Result<(Arc<Platform>, super::super::tests::TempDir)> {
        let (command, _) = mpsc::channel(1);
        let config = PlatformConfig {
            agent_port: Some(0),
            ..Default::default()
        };
        let (platform, _, dir) = platform_with(&config, command).await?;
        Ok((Arc::new(platform), dir))
    }

    fn target_request(name: &str) -> TargetRequest {
        TargetRequest {
            name: name.parse().unwrap(),
        }
    }

    #[tokio::test]
    async fn a_target_needs_the_agent_port() -> anyhow::Result<()> {
        let (command, _) = mpsc::channel(1);
        let (platform, _, _dir) = platform_with(&PlatformConfig::default(), command).await?;
        let err = platform
            .add_target(target_request("edge"), 1)
            .await
            .unwrap_err();
        assert!(matches!(api_error(err), Error::AgentPortMissing));
        Ok(())
    }

    #[tokio::test]
    async fn a_target_is_added_with_a_token_that_a_new_token_replaces() -> anyhow::Result<()> {
        let (platform, _dir) = agent_platform().await?;
        let added = platform.add_target(target_request("edge"), 1).await?;
        let target = &added.target;
        assert_eq!(target.kind, TargetKind::Agent);
        assert!(!target.enrolled && !target.online);
        let pki = platform.agent_pki()?;
        let token: EnrollmentToken = added.token.parse()?;
        assert_eq!(token.ca_hash, pki.ca_hash());

        let err = platform
            .add_target(target_request("edge"), 2)
            .await
            .unwrap_err();
        assert!(matches!(api_error(err), Error::TargetNameExists { .. }));

        let renewed = platform.new_target_token(target.id).await?;
        let renewed: EnrollmentToken = renewed.token.parse()?;
        let store = &platform.store;
        assert_eq!(store.target_by_token(&token.secret_hash()).await?, None);
        let found = store.target_by_token(&renewed.secret_hash()).await?;
        assert_eq!(found, Some(target.id));
        Ok(())
    }

    #[tokio::test]
    async fn the_local_target_stays() -> anyhow::Result<()> {
        let (platform, _dir) = agent_platform().await?;
        let local: ShortId = LOCAL_TARGET.parse()?;
        let err = platform.new_target_token(local).await.unwrap_err();
        assert!(matches!(api_error(err), Error::TargetReadOnly { .. }));
        let err = platform.delete_target(local).await.unwrap_err();
        assert!(matches!(api_error(err), Error::TargetReadOnly { .. }));
        let err = platform
            .delete_target("bcd-fgh".parse()?)
            .await
            .unwrap_err();
        assert!(matches!(api_error(err), Error::IdNotFound { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn a_target_with_an_app_is_not_deleted() -> anyhow::Result<()> {
        let (platform, _dir) = agent_platform().await?;
        let edge = platform.add_target(target_request("edge"), 1).await?.target;
        let mut shop = request("shop");
        shop.target = edge.id;
        let shop = platform.add_app(shop, 1).await?;
        let err = platform.delete_target(edge.id).await.unwrap_err();
        assert!(matches!(api_error(err), Error::TargetInUse { .. }));

        platform.delete_app(shop.id).await?;
        assert_eq!(platform.delete_target(edge.id).await?.name, "edge");
        let ids = platform.targets().await?.into_iter().map(|t| t.id);
        assert_eq!(ids.collect::<Vec<_>>(), vec![LOCAL_TARGET.parse()?]);
        Ok(())
    }

    #[tokio::test]
    async fn an_agent_enrolls_with_the_token_of_its_target() -> anyhow::Result<()> {
        let (command, _) = mpsc::channel(1);
        let config = PlatformConfig {
            agent_port: Some(0),
            ..Default::default()
        };
        let (platform, _, _dir) = platform_with(&config, command).await?;
        let pki = platform.pki.as_ref().context("pki")?;
        let token = EnrollmentToken::new(pki.ca_hash());
        let edge: ShortId = "fzn-txd".parse()?;
        platform
            .store
            .add_agent_target(edge, "edge", &token.secret_hash(), 1)
            .await?;

        let (_, csr) = new_agent_key()?;
        let request = EnrollRequest {
            secret: token.secret.clone(),
            csr,
            version: "1.0.0".into(),
        };
        let client = IpAddr::from([192, 0, 2, 7]);
        let enrolled = platform.enroll(&request, client).await?;
        assert_eq!(enrolled.target, edge);
        let signed = pem_certificates(&enrolled.certificate)?;
        assert_eq!(
            platform.target_of(&fingerprint(&signed[0])).await?,
            Some(edge)
        );

        let again = platform.enroll(&request, client).await.unwrap_err();
        assert!(matches!(
            again.downcast_ref::<Error>(),
            Some(Error::InvalidEnrollmentToken)
        ));
        Ok(())
    }
}
