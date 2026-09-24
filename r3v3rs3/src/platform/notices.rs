//! The notifications of the platform: the deployment events and the connection events of the
//! agents. The server loop sends them to the webhook of the settings.

use super::Platform;
use super::store::{UNFINISHED_MESSAGE, UnfinishedDeployment};
use crate::command::ServerCommand;
use crate::notify::{DeploymentSummary, Notification, NotificationEvent, TargetSummary};
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::DeploymentTrigger;
use tokio::sync::mpsc;
use tracing::warn;

/// The deployment of a notification.
pub(super) struct DeploymentRef<'a> {
    pub id: ShortId,
    pub app: ShortId,
    pub app_name: &'a AppName,
    pub trigger: DeploymentTrigger,
}

impl DeploymentRef<'_> {
    fn summary(&self) -> DeploymentSummary {
        DeploymentSummary {
            id: self.id,
            app: self.app,
            app_name: self.app_name.to_string(),
            trigger: self.trigger.as_str(),
        }
    }
}

impl Platform {
    /// Sends a deployment event to the server. Never call it from the server loop, because it
    /// waits for room in the command channel.
    pub(super) async fn notify_deployment(
        &self,
        event: NotificationEvent,
        deployment: DeploymentRef<'_>,
        error: Option<String>,
    ) {
        let notification = Notification::deployment(event, deployment.summary(), error);
        send(&self.command, notification).await;
    }

    /// Sends the connection event of the agent of a target to the server.
    pub(super) async fn notify_agent(&self, event: NotificationEvent, target: ShortId) {
        let name = match self.target(target).await {
            Ok(entry) => entry.name,
            // A deleted target has no events.
            Err(err) if is_not_found(&err) => return,
            Err(err) => {
                warn!(%target, "failed to read the target of an agent event: {err:#}");
                target.to_string()
            }
        };
        let notification = Notification::agent(event, TargetSummary { id: target, name });
        send(&self.command, notification).await;
    }
}

/// Sends a failure event for each deployment that a restart failed. The platform opens before
/// the server loop runs, so a task sends them and waits for the loop.
pub(super) fn notify_unfinished(
    command: &mpsc::Sender<ServerCommand>,
    failed: Vec<UnfinishedDeployment>,
) {
    if failed.is_empty() {
        return;
    }
    let command = command.clone();
    tokio::spawn(async move {
        for deployment in failed {
            let notice = DeploymentRef {
                id: deployment.id,
                app: deployment.app,
                app_name: &deployment.app_name,
                trigger: deployment.trigger,
            };
            let error = Some(UNFINISHED_MESSAGE.to_string());
            let event = NotificationEvent::DeploymentFailed;
            send(
                &command,
                Notification::deployment(event, notice.summary(), error),
            )
            .await;
        }
    });
}

fn is_not_found(err: &anyhow::Error) -> bool {
    matches!(err.downcast_ref(), Some(Error::IdNotFound { .. }))
}

pub(super) async fn send(command: &mpsc::Sender<ServerCommand>, notification: Notification) {
    let event = notification.event;
    if command
        .send(ServerCommand::Notify { notification })
        .await
        .is_err()
    {
        warn!(?event, "the server stopped before it got the notification");
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::FakeRuntime;
    use super::super::tests::{TempDir, platform_with, request};
    use super::*;
    use r3v3rs3_api::platform::{DeploymentStatus, PlatformConfig};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    type Commands = mpsc::Receiver<ServerCommand>;

    struct Setup {
        platform: Arc<Platform>,
        runtime: Arc<FakeRuntime>,
        commands: Commands,
        app: ShortId,
        _dir: TempDir,
    }

    /// A server on 127.0.0.1 that answers every request with 200, and its port.
    async fn http_ok() -> anyhow::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });
        Ok(port)
    }

    /// A platform with the image app `shop`, whose container passes its health check.
    async fn setup() -> anyhow::Result<Setup> {
        let (command, commands) = mpsc::channel(16);
        let (platform, runtime, dir) = platform_with(&PlatformConfig::default(), command).await?;
        runtime.state().host_port = http_ok().await?;
        let mut shop = request("shop");
        shop.spec.health_check_path = Some("/".into());
        let app = platform.add_app(shop, 1).await?;
        Ok(Setup {
            platform: Arc::new(platform),
            runtime,
            commands,
            app: app.id,
            _dir: dir,
        })
    }

    /// The deployment events until the first final one.
    async fn deployment_events(commands: &mut Commands) -> anyhow::Result<Vec<Notification>> {
        let mut events = Vec::new();
        while let Some(command) =
            tokio::time::timeout(Duration::from_secs(5), commands.recv()).await?
        {
            let ServerCommand::Notify { notification } = command else {
                continue;
            };
            let last = notification.event != NotificationEvent::DeploymentStarted;
            events.push(notification);
            if last {
                break;
            }
        }
        Ok(events)
    }

    fn kinds(events: &[Notification]) -> Vec<NotificationEvent> {
        events.iter().map(|n| n.event).collect()
    }

    #[tokio::test]
    async fn a_deployment_reports_its_start_and_its_end() -> anyhow::Result<()> {
        let Setup {
            platform,
            runtime,
            mut commands,
            app,
            _dir,
        } = setup().await?;
        let trigger = DeploymentTrigger::Manual;
        let first = platform.deploy(app, "alice", trigger).await?;
        let events = deployment_events(&mut commands).await?;
        use NotificationEvent::*;
        assert_eq!(kinds(&events), [DeploymentStarted, DeploymentRunning]);
        let summary = events[1].deployment.clone().unwrap();
        assert_eq!((summary.id, summary.app), (first.id, app));
        assert_eq!(
            (summary.app_name.as_str(), summary.trigger),
            ("shop", "manual")
        );

        runtime.state().crash = true;
        while platform.busy_apps().contains(&app) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        platform.deploy(app, "alice", trigger).await?;
        let events = deployment_events(&mut commands).await?;
        assert_eq!(kinds(&events), [DeploymentStarted, DeploymentFailed]);
        assert!(events[1].error.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn a_restart_reports_the_deployments_that_it_failed() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (platform, app) = (&setup.platform, setup.app);
        // A deployment that no pipeline runs stays queued, like after a stop of the server.
        let entry = platform.app(app).await?;
        let id = super::super::new_id()?;
        let queued = crate::platform::store::NewDeployment {
            id,
            app,
            trigger: DeploymentTrigger::Webhook,
            username: "webhook",
            spec: &entry.spec,
            env: &[],
            image_digest: None,
            started_at: 2,
        };
        platform.store.insert_deployment(&queued).await?;
        let failed = platform.store.fail_unfinished(9).await?;
        assert_eq!(failed.len(), 1);
        assert_eq!((failed[0].id, failed[0].app_name.as_str()), (id, "shop"));
        assert_eq!(failed[0].trigger, DeploymentTrigger::Webhook);
        let stored = platform.deployment(id).await?;
        assert_eq!(stored.status, DeploymentStatus::Failed);

        let (command, mut commands) = mpsc::channel(4);
        notify_unfinished(&command, failed);
        let Some(ServerCommand::Notify { notification }) = commands.recv().await else {
            anyhow::bail!("no notification");
        };
        assert_eq!(notification.event, NotificationEvent::DeploymentFailed);
        assert_eq!(notification.error.as_deref(), Some(UNFINISHED_MESSAGE));
        Ok(())
    }

    #[tokio::test]
    async fn an_agent_event_names_its_target_and_a_deleted_target_has_none() -> anyhow::Result<()> {
        let Setup {
            platform,
            mut commands,
            _dir,
            ..
        } = setup().await?;
        let local: ShortId = r3v3rs3_api::platform::LOCAL_TARGET.parse()?;
        platform
            .notify_agent(NotificationEvent::AgentOnline, local)
            .await;
        let Some(ServerCommand::Notify { notification }) = commands.recv().await else {
            anyhow::bail!("no notification");
        };
        let target = notification.target.unwrap();
        assert_eq!((target.id, target.name.as_str()), (local, "local"));

        platform
            .notify_agent(NotificationEvent::AgentOffline, "bcd-fgh".parse()?)
            .await;
        assert!(commands.try_recv().is_err());
        Ok(())
    }
}
