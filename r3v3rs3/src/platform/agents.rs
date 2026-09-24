//! The agent targets of the platform: the enrollment of an agent and the port that the agents
//! connect to.

use super::Platform;
use crate::agent::listener::{AgentDirectory, AgentListener, LinkTiming};
use crate::agent::pki::{fingerprint, pem_certificates};
use crate::agent::protocol::{EnrollRequest, Enrolled};
use crate::agent::token::secret_hash;
use anyhow::Context;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
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
}

#[async_trait::async_trait]
impl AgentDirectory for Platform {
    async fn target_of(&self, fingerprint: &str) -> anyhow::Result<Option<ShortId>> {
        self.store.target_by_fingerprint(fingerprint).await
    }

    async fn enroll(&self, request: &EnrollRequest) -> anyhow::Result<Enrolled> {
        let pki = self.pki.as_ref().context("the agent CA is not loaded")?;
        let token_hash = secret_hash(&request.secret);
        let target = self
            .store
            .target_by_token(&token_hash)
            .await?
            .ok_or(Error::InvalidEnrollmentToken)?;
        let certificate = pki.sign(&request.csr, target)?;
        let signed = pem_certificates(&certificate)?;
        let signed = signed.first().context("the signed certificate is empty")?;
        if !self
            .store
            .enroll_target(target, &token_hash, &fingerprint(signed))
            .await?
        {
            return Err(Error::InvalidEnrollmentToken.into());
        }
        // The certificate of an earlier enrollment no longer matches the target.
        self.agents.disconnect(target);
        Ok(Enrolled {
            target,
            certificate,
            ca: pki.ca_pem()?,
        })
    }

    async fn seen(&self, target: ShortId, now: u64) -> anyhow::Result<()> {
        self.store.target_seen(target, now).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::platform_with;
    use super::*;
    use crate::agent::pki::new_agent_key;
    use crate::agent::token::EnrollmentToken;
    use r3v3rs3_api::platform::PlatformConfig;
    use tokio::sync::mpsc;

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
        let enrolled = platform.enroll(&request).await?;
        assert_eq!(enrolled.target, edge);
        let signed = pem_certificates(&enrolled.certificate)?;
        assert_eq!(
            platform.target_of(&fingerprint(&signed[0])).await?,
            Some(edge)
        );

        let again = platform.enroll(&request).await.unwrap_err();
        assert!(matches!(
            again.downcast_ref::<Error>(),
            Some(Error::InvalidEnrollmentToken)
        ));
        Ok(())
    }
}
