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
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, warn};

/// The attempts of one notification.
const ATTEMPTS: u32 = 3;
/// The wait before the second attempt. Each later attempt waits one more step.
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// The notifications that wait for delivery. A full queue drops a new notification.
const QUEUE_SIZE: usize = 64;

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
}

impl NotificationEvent {
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
    pub error: Option<String>,
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
            error: None,
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
/// are new.
pub fn certificate_notifications(
    certs: &[CertInfo],
    sent: &HashSet<String>,
    now: i64,
    warning: Duration,
    node: &str,
) -> (Vec<Notification>, HashSet<String>) {
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
            notifications.push(Notification::new(event, node).certificate(info));
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

/// Sends the notifications in the background, so a slow webhook does not delay the server.
#[derive(Clone)]
pub struct Notifier {
    sender: mpsc::Sender<(WebhookConfig, Notification)>,
}

impl Notifier {
    pub fn spawn() -> Self {
        let (sender, mut receiver) = mpsc::channel::<(WebhookConfig, Notification)>(QUEUE_SIZE);
        tokio::spawn(async move {
            while let Some((webhook, notification)) = receiver.recv().await {
                deliver_with_retry(&webhook, &notification).await;
            }
        });
        Self { sender }
    }

    /// Queues the notification. A full queue drops it and logs an error.
    pub fn send(&self, webhook: &WebhookConfig, notification: Notification) {
        if let Err(err) = self.sender.try_send((webhook.clone(), notification)) {
            error!(%err, "failed to queue the notification");
        }
    }
}

async fn deliver_with_retry(webhook: &WebhookConfig, notification: &Notification) {
    for attempt in 1..=ATTEMPTS {
        let Err(err) = deliver(webhook, notification).await else {
            return;
        };
        if attempt == ATTEMPTS {
            error!(event = ?notification.event, "failed to send the notification: {err:#}");
            return;
        }
        warn!(event = ?notification.event, attempt, "failed to send the notification: {err:#}");
        tokio::time::sleep(RETRY_DELAY * attempt).await;
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
            .map(|notification| (notification.event, notification.certificate.clone()))
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
}
