//! The notifications about certificates, ACME orders, deployments and agents that r3v3rs3 sends
//! to the webhook of the settings.

use crate::cdn::fetch::build_client;
use crate::certs::acme::AcmeTarget;
use crate::certs::dns::api::{ApiClient, ApiRequest};
use crate::clock::unix_ms;
use anyhow::{anyhow, bail};
use hyper::Method;
use r3v3rs3_api::acme::webhook_url_allowed;
use r3v3rs3_api::app::WebhookConfig;
use r3v3rs3_api::cert::{CertInfo, ExpiryState, expiry_state};
use r3v3rs3_api::id::ShortId;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};
use tracing::{error, warn};

/// The attempts of one notification.
const ATTEMPTS: u32 = 3;
/// The wait before the second attempt. Each later attempt waits one more step.
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// The notifications that wait for delivery.
const QUEUE_SIZE: usize = 1024;
/// The maximum concurrent notification deliveries across all destinations.
const MAX_CONCURRENT_DELIVERIES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEvent {
    CertificateExpiring,
    CertificateExpired,
    AcmeOrderFailed,
    Test,
    DeploymentStarted,
    DeploymentRunning,
    DeploymentFailed,
    AgentOnline,
    AgentOffline,
    AppAlert,
}

impl NotificationEvent {
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            Self::CertificateExpiring
                | Self::CertificateExpired
                | Self::AcmeOrderFailed
                | Self::DeploymentFailed
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::CertificateExpiring => "certificate_expiring",
            Self::CertificateExpired => "certificate_expired",
            Self::AcmeOrderFailed => "acme_order_failed",
            Self::Test => "test",
            Self::DeploymentStarted => "deployment_started",
            Self::DeploymentRunning => "deployment_running",
            Self::DeploymentFailed => "deployment_failed",
            Self::AgentOnline => "agent_online",
            Self::AgentOffline => "agent_offline",
            Self::AppAlert => "app_alert",
        }
    }
}

/// The JSON body of a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notification {
    pub event: NotificationEvent,
    /// The Unix time in seconds.
    pub time: u64,
    /// The name of the cluster node. Empty without a cluster.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<CertificateSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acme: Option<AcmeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<DeploymentSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert: Option<Box<AppAlertSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An app resource threshold alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppAlertSummary {
    pub app: ShortId,
    pub app_name: String,
    pub metric: String,
    pub current_value: String,
    pub threshold: String,
}

/// A deployment of the platform, with the app that it deploys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentSummary {
    pub id: ShortId,
    pub app: ShortId,
    pub app_name: String,
    /// `manual`, `webhook`, `rollback` or `api`.
    pub trigger: &'static str,
}

/// An agent target of the platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetSummary {
    pub id: ShortId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CertificateSummary {
    pub id: ShortId,
    pub san: Vec<String>,
    /// The Unix time in seconds.
    pub not_after: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcmeSummary {
    pub id: ShortId,
    pub identifiers: Vec<String>,
}

impl Notification {
    pub fn new(event: NotificationEvent, node: &str) -> Self {
        Self {
            event,
            time: unix_ms() / 1000,
            node: node.to_string(),
            certificate: None,
            acme: None,
            deployment: None,
            target: None,
            alert: None,
            error: None,
        }
    }

    /// An app resource threshold alert event.
    pub fn app_alert(
        app: ShortId,
        app_name: String,
        metric: &'static str,
        current_value: String,
        threshold: String,
    ) -> Self {
        Self {
            alert: Some(Box::new(AppAlertSummary {
                app,
                app_name,
                metric: metric.to_string(),
                current_value,
                threshold,
            })),
            ..Self::new(NotificationEvent::AppAlert, "")
        }
    }

    /// A deployment event. A failed deployment carries its message in `error`. The platform
    /// never runs in a cluster, so the notification names no node.
    pub fn deployment(
        event: NotificationEvent,
        deployment: DeploymentSummary,
        error: Option<String>,
    ) -> Self {
        Self {
            deployment: Some(deployment),
            error,
            ..Self::new(event, "")
        }
    }

    /// The connection event of an agent.
    pub fn agent(event: NotificationEvent, target: TargetSummary) -> Self {
        Self {
            target: Some(target),
            ..Self::new(event, "")
        }
    }

    fn certificate(mut self, info: &CertInfo) -> Self {
        self.certificate = Some(CertificateSummary {
            id: info.id,
            san: info.san.iter().map(ToString::to_string).collect(),
            not_after: info.not_after,
        });
        self
    }

    /// The notification of an ACME order that failed with `error`.
    pub fn acme_order_failed(node: &str, target: &AcmeTarget, error: String) -> Self {
        Self {
            acme: Some(AcmeSummary {
                id: target.acme_id,
                identifiers: target.identifiers.clone(),
            }),
            error: Some(error),
            ..Self::new(NotificationEvent::AcmeOrderFailed, node)
        }
    }
}

/// The certificate notifications to send at the Unix time `now`, and the keys of the events that
/// the certificates have now. `sent` holds the keys of the events that were sent before, so each
/// event of a certificate is sent once. A renewed certificate has a new fingerprint, so its events
/// are new. Each notification is paired with its event key.
pub fn certificate_notifications(
    certs: &[CertInfo],
    sent: &HashSet<String>,
    now: i64,
    warning: Duration,
    node: &str,
) -> (Vec<(Notification, String)>, HashSet<String>) {
    let mut notifications = Vec::new();
    let mut keys = HashSet::new();
    for info in certs {
        let event = match expiry_state(info.not_after, now, warning) {
            ExpiryState::Valid => continue,
            ExpiryState::Expiring => NotificationEvent::CertificateExpiring,
            ExpiryState::Expired => NotificationEvent::CertificateExpired,
        };
        let key = format!("{}:{}", event.as_str(), info.fingerprint);
        if !sent.contains(&key) {
            notifications.push((
                Notification::new(event, node).certificate(info),
                key.clone(),
            ));
        }
        keys.insert(key);
    }
    (notifications, keys)
}

/// Sends the notification once. The error names the status and the start of the response body,
/// never the token.
pub async fn deliver(webhook: &WebhookConfig, notification: &Notification) -> anyhow::Result<()> {
    if !webhook_url_allowed(&webhook.url) {
        bail!("the webhook URL must use https, or http on a loopback address");
    }
    let api = ApiClient::new(build_client().await?, None, webhook.url.trim())?;
    let body = serde_json::to_value(notification)?;
    let mut request = ApiRequest::new(Method::POST, String::new()).json(&body);
    if let Some(token) = webhook.token.as_deref().filter(|token| !token.is_empty()) {
        request = request.bearer(token);
    }
    tokio::time::timeout(webhook.timeout, api.send(request))
        .await
        .map_err(|_| anyhow!("the webhook did not answer in {:?}", webhook.timeout))??;
    Ok(())
}

type SuccessCallback = Box<dyn FnOnce() + Send + 'static>;

struct NotificationTask {
    webhook: WebhookConfig,
    notification: Notification,
    on_success: Option<SuccessCallback>,
}

/// Sends the notifications in the background, so a slow webhook does not delay the server.
#[derive(Clone)]
pub struct Notifier {
    sender: mpsc::Sender<NotificationTask>,
}

impl Notifier {
    pub fn spawn() -> Self {
        let (sender, mut receiver) = mpsc::channel::<NotificationTask>(QUEUE_SIZE);
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DELIVERIES));
        tokio::spawn(async move {
            while let Some(task) = receiver.recv().await {
                let sem = semaphore.clone();
                let permit = match sem.acquire_owned().await {
                    Ok(permit) => permit,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    deliver_with_retry(&task.webhook, &task.notification, task.on_success).await;
                });
            }
        });
        Self { sender }
    }

    fn enqueue_or_spawn(&self, task: NotificationTask) {
        let is_critical = task.notification.event.is_critical();
        match self.sender.try_send(task) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(task)) => {
                if is_critical {
                    warn!(
                        event = ?task.notification.event,
                        "notification queue full; dispatching critical alert in background task"
                    );
                    tokio::spawn(async move {
                        deliver_with_retry(&task.webhook, &task.notification, task.on_success)
                            .await;
                    });
                } else {
                    error!(
                        event = ?task.notification.event,
                        "notification queue full; non-critical notification dropped"
                    );
                }
            }
            Err(mpsc::error::TrySendError::Closed(task)) => {
                error!(
                    event = ?task.notification.event,
                    "notification channel closed"
                );
            }
        }
    }

    /// Queues the notification. Critical alerts fall back to direct background dispatch on full queue.
    pub fn send(&self, webhook: &WebhookConfig, notification: Notification) {
        self.enqueue_or_spawn(NotificationTask {
            webhook: webhook.clone(),
            notification,
            on_success: None,
        });
    }

    /// Queues the notification and calls `on_success` only after delivery succeeds.
    pub fn send_with_success<F>(
        &self,
        webhook: &WebhookConfig,
        notification: Notification,
        on_success: F,
    ) where
        F: FnOnce() + Send + 'static,
    {
        self.enqueue_or_spawn(NotificationTask {
            webhook: webhook.clone(),
            notification,
            on_success: Some(Box::new(on_success)),
        });
    }
}

async fn deliver_with_retry(
    webhook: &WebhookConfig,
    notification: &Notification,
    on_success: Option<SuccessCallback>,
) {
    for attempt in 1..=ATTEMPTS {
        match deliver(webhook, notification).await {
            Ok(()) => {
                if let Some(callback) = on_success {
                    callback();
                }
                return;
            }
            Err(err) => {
                if attempt == ATTEMPTS {
                    error!(event = ?notification.event, "failed to send the notification: {err:#}");
                    return;
                }
                warn!(event = ?notification.event, attempt, "failed to send the notification: {err:#}");
                tokio::time::sleep(RETRY_DELAY * attempt).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::cert::CertKind;
    use serde_json::json;

    const DAY: i64 = 24 * 60 * 60;

    fn cert(fingerprint: &str, not_after: i64) -> CertInfo {
        CertInfo {
            id: "web".parse().unwrap(),
            kind: CertKind::Server,
            fingerprint: fingerprint.into(),
            issuer: String::new(),
            root_cert: None,
            san: vec!["example.com".parse().unwrap()],
            not_after,
            not_before: 0,
            is_ca: false,
            has_private_key: true,
            metadata: None,
            source: None,
        }
    }

    fn keys(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn each_event_of_a_certificate_is_sent_once() {
        let warning = Duration::from_secs(DAY as u64);
        let now = 1_000_000;
        let certs = [
            cert("valid", now + 30 * DAY),
            cert("soon", now + 3_600),
            cert("old", now - 1),
        ];
        let (sent, current) = certificate_notifications(&certs, &HashSet::new(), now, warning, "a");
        let events = sent
            .iter()
            .map(|(notification, _)| (notification.event, notification.certificate.clone()))
            .map(|(event, certificate)| (event, certificate.map(|c| c.not_after)))
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            [
                (NotificationEvent::CertificateExpiring, Some(now + 3_600)),
                (NotificationEvent::CertificateExpired, Some(now - 1)),
            ]
        );
        assert_eq!(
            current,
            keys(&["certificate_expiring:soon", "certificate_expired:old"])
        );

        // A second run sends nothing, and a renewed certificate has new events.
        let (again, same) = certificate_notifications(&certs, &current, now, warning, "a");
        assert!(again.is_empty());
        assert_eq!(same, current);
        let renewed = [cert("renewed", now + 3_600)];
        let (sent, current) = certificate_notifications(&renewed, &current, now, warning, "a");
        assert_eq!(sent.len(), 1);
        assert_eq!(current, keys(&["certificate_expiring:renewed"]));
    }

    #[test]
    fn a_notification_names_its_event_and_leaves_out_the_empty_parts() {
        let target = AcmeTarget::new("acme".parse().unwrap(), ["example.com"]);
        let mut failed = Notification::acme_order_failed("", &target, "rate limited".into());
        failed.time = 5;
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            json!({
                "event": "acme_order_failed",
                "time": 5,
                "acme": {"id": "acme", "identifiers": ["example.com"]},
                "error": "rate limited",
            })
        );

        let mut expiring = Notification::new(NotificationEvent::CertificateExpiring, "node-a")
            .certificate(&cert("f", 7));
        expiring.time = 5;
        assert_eq!(
            serde_json::to_value(&expiring).unwrap(),
            json!({
                "event": "certificate_expiring",
                "time": 5,
                "node": "node-a",
                "certificate": {"id": "web", "san": ["example.com"], "not_after": 7},
            })
        );
    }

    #[test]
    fn platform_notifications_name_the_deployment_or_the_target() {
        let summary = DeploymentSummary {
            id: "dep".parse().unwrap(),
            app: "app".parse().unwrap(),
            app_name: "shop".into(),
            trigger: "webhook",
        };
        let event = NotificationEvent::DeploymentFailed;
        let mut failed = Notification::deployment(event, summary, Some("no image".into()));
        failed.time = 5;
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            json!({
                "event": "deployment_failed",
                "time": 5,
                "deployment": {"id": "dep", "app": "app", "app_name": "shop", "trigger": "webhook"},
                "error": "no image",
            })
        );

        let target = TargetSummary {
            id: "edge".parse().unwrap(),
            name: "edge-1".into(),
        };
        let mut offline = Notification::agent(NotificationEvent::AgentOffline, target);
        offline.time = 5;
        assert_eq!(
            serde_json::to_value(&offline).unwrap(),
            json!({
                "event": "agent_offline",
                "time": 5,
                "target": {"id": "edge", "name": "edge-1"},
            })
        );
    }

    #[tokio::test]
    async fn critical_notifications_dispatch_even_when_queue_full() {
        let (sender, _receiver) = mpsc::channel::<NotificationTask>(1);
        let notifier = Notifier { sender };
        let webhook = WebhookConfig {
            url: "http://127.0.0.1:9/nowhere".to_string(),
            timeout: Duration::from_millis(50),
            ..Default::default()
        };
        // Fill the 1-slot channel
        notifier.send(
            &webhook,
            Notification::new(NotificationEvent::AppAlert, "node-1"),
        );

        // Sending a critical notification when channel is full dispatches directly without panicking
        notifier.send(
            &webhook,
            Notification::new(NotificationEvent::CertificateExpired, "node-1"),
        );
    }
}
