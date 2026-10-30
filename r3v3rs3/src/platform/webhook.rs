//! Outbound webhooks and event notification delivery engine.

use super::Platform;
use super::store::StoredWebhook;
use crate::cdn::fetch::{HttpClient, build_client};
use crate::clock::unix_secs;
use bytes::Bytes;
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::Full;
use hyper::header::{CONTENT_TYPE, USER_AGENT};
use hyper::{Method, Request};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::webhook::{
    CreateWebhookRequest, OutboundWebhook, TestWebhookRequest, TestWebhookResponse,
    UpdateWebhookRequest, WebhookEvent, WebhookPayload,
};
use sha2::Sha256;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tracing::{error, info, warn};

pub(crate) fn webhook_aad(id: ShortId) -> String {
    format!("outbound_webhook:{id}")
}

pub(crate) fn sign_payload(secret: &[u8], body: &[u8]) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return String::new();
    };
    mac.update(body);
    let signature = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(signature))
}

pub fn build_payload(
    event: WebhookEvent,
    summary: &str,
    data: serde_json::Value,
) -> WebhookPayload {
    let now = OffsetDateTime::now_utc();
    let created_at = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| unix_secs().to_string());
    WebhookPayload {
        id: format!("evt_{}", ShortId::new()),
        event,
        created_at,
        timestamp: now.unix_timestamp(),
        summary: summary.to_string(),
        data,
    }
}

pub async fn deliver_webhook(
    client: &HttpClient,
    url: &str,
    secret: Option<&[u8]>,
    payload: &WebhookPayload,
) -> anyhow::Result<(u16, u64)> {
    let raw_body = serde_json::to_vec(payload)?;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(
            USER_AGENT,
            concat!("r3v3rs3-webhook/", env!("CARGO_PKG_VERSION")),
        )
        .header("X-R3v3rs3-Event", payload.event.as_str())
        .header("X-R3v3rs3-Delivery", &payload.id);

    if let Some(sec) = secret {
        let sig = sign_payload(sec, &raw_body);
        builder = builder.header("X-R3v3rs3-Signature", sig);
    }

    let req = builder.body(Full::new(Bytes::from(raw_body)))?;
    let start = Instant::now();
    let resp = tokio::time::timeout(Duration::from_secs(10), client.request(req))
        .await
        .map_err(|_| anyhow::anyhow!("request timed out after 10s"))??;

    let status = resp.status().as_u16();
    let elapsed = start.elapsed().as_millis() as u64;
    Ok((status, elapsed))
}

pub async fn test_webhook_url(
    url: &str,
    secret: Option<&str>,
    event: WebhookEvent,
) -> TestWebhookResponse {
    let client = match build_client().await {
        Ok(c) => c,
        Err(err) => {
            return TestWebhookResponse {
                success: false,
                status_code: None,
                message: format!("failed to initialize HTTP client: {err}"),
                elapsed_ms: 0,
            };
        }
    };

    let payload = build_payload(
        event,
        "This is a test notification from r3v3rs3 webhook center.",
        serde_json::json!({
            "test": true,
            "message": "Hello from r3v3rs3!",
            "version": env!("CARGO_PKG_VERSION"),
        }),
    );

    match deliver_webhook(&client, url, secret.map(|s| s.as_bytes()), &payload).await {
        Ok((status, elapsed)) => {
            let success = (200..300).contains(&status);
            TestWebhookResponse {
                success,
                status_code: Some(status),
                message: if success {
                    format!("Delivered successfully (HTTP {status})")
                } else {
                    format!("Server returned HTTP status {status}")
                },
                elapsed_ms: elapsed,
            }
        }
        Err(err) => TestWebhookResponse {
            success: false,
            status_code: None,
            message: format!("Delivery error: {err}"),
            elapsed_ms: 0,
        },
    }
}

async fn deliver_to_all(targets: Vec<(String, String, Option<Vec<u8>>)>, payload: WebhookPayload) {
    let client = match build_client().await {
        Ok(c) => c,
        Err(err) => {
            error!(%err, "failed to build HTTP client for webhook delivery");
            return;
        }
    };

    for (name, url, secret) in targets {
        if r3v3rs3_api::webhook::validate_webhook_url(&url).is_err() {
            warn!(
                webhook = %name,
                url = %url,
                "skipping delivery to restricted webhook URL"
            );
            continue;
        }
        match deliver_webhook(&client, &url, secret.as_deref(), &payload).await {
            Ok((status, elapsed)) => {
                info!(
                    webhook = %name,
                    url = %url,
                    status,
                    elapsed_ms = elapsed,
                    event = %payload.event,
                    "webhook delivered"
                );
            }
            Err(err) => {
                warn!(
                    webhook = %name,
                    url = %url,
                    %err,
                    event = %payload.event,
                    "webhook delivery failed"
                );
            }
        }
    }
}

fn apply_field_updates(stored: &mut StoredWebhook, request: &UpdateWebhookRequest) {
    if let Some(ref name) = request.name {
        stored.name = name.trim().to_string();
    }
    if let Some(ref url) = request.url {
        stored.url = url.trim().to_string();
    }
    if let Some(enabled) = request.enabled {
        stored.enabled = enabled;
    }
    if let Some(ref events) = request.events {
        stored.events = events.clone();
    }
    stored.updated_at = unix_secs();
}

fn seal_updated_secret(
    keys: &crate::keys::crypto::ClusterKeys,
    id: ShortId,
    secret_opt: Option<&str>,
) -> Result<Option<Option<Vec<u8>>>, Error> {
    let Some(sec) = secret_opt else {
        return Ok(None);
    };
    let trimmed = sec.trim();
    if trimmed.is_empty() {
        Ok(Some(None))
    } else {
        let aad = webhook_aad(id);
        let sealed = keys
            .seal(&aad, trimmed.as_bytes())
            .map_err(|err| Error::InvalidWebhook {
                reason: err.to_string(),
            })?;
        Ok(Some(Some(sealed)))
    }
}

impl Platform {
    /// Lists all outbound webhooks.
    pub async fn list_outbound_webhooks(&self) -> Result<Vec<OutboundWebhook>, Error> {
        let list = self
            .store
            .list_webhooks()
            .await
            .map_err(|_err| Error::FailedToFetchLog)?;
        Ok(list.into_iter().map(OutboundWebhook::from).collect())
    }

    /// Gets an outbound webhook by ID.
    pub async fn get_outbound_webhook(&self, id: ShortId) -> Result<OutboundWebhook, Error> {
        let stored = self
            .store
            .webhook(id)
            .await
            .map_err(|_| Error::WebhookNotFound { id })?
            .ok_or(Error::WebhookNotFound { id })?;
        Ok(OutboundWebhook::from(stored))
    }

    /// Creates a new outbound webhook.
    pub async fn create_outbound_webhook(
        &self,
        request: CreateWebhookRequest,
    ) -> Result<OutboundWebhook, Error> {
        request.validate()?;
        let id = ShortId::new();
        let now = unix_secs();

        let sealed_secret = if let Some(ref sec) = request.secret {
            let trimmed = sec.trim();
            if !trimmed.is_empty() {
                let aad = webhook_aad(id);
                Some(self.keys().seal(&aad, trimmed.as_bytes()).map_err(|err| {
                    Error::InvalidWebhook {
                        reason: err.to_string(),
                    }
                })?)
            } else {
                None
            }
        } else {
            None
        };

        let stored = StoredWebhook {
            id,
            name: request.name.trim().to_string(),
            url: request.url.trim().to_string(),
            has_secret: sealed_secret.is_some(),
            enabled: request.enabled,
            events: request.events,
            created_at: now,
            updated_at: now,
        };

        self.store
            .insert_webhook(&stored, sealed_secret.as_deref())
            .await
            .map_err(|err| Error::InvalidWebhook {
                reason: err.to_string(),
            })?;

        Ok(OutboundWebhook::from(stored))
    }

    /// Updates an existing outbound webhook.
    pub async fn update_outbound_webhook(
        &self,
        id: ShortId,
        request: UpdateWebhookRequest,
    ) -> Result<OutboundWebhook, Error> {
        request.validate()?;
        let mut stored = self
            .store
            .webhook(id)
            .await
            .map_err(|_| Error::WebhookNotFound { id })?
            .ok_or(Error::WebhookNotFound { id })?;

        apply_field_updates(&mut stored, &request);
        let sealed_secret = seal_updated_secret(&self.keys(), id, request.secret.as_deref())?;

        if let Some(ref opt) = sealed_secret {
            stored.has_secret = opt.is_some();
        }

        let secret_ref = sealed_secret.as_ref().map(|opt| opt.as_deref());
        self.store
            .update_webhook(&stored, secret_ref)
            .await
            .map_err(|err| Error::InvalidWebhook {
                reason: err.to_string(),
            })?;

        Ok(OutboundWebhook::from(stored))
    }

    /// Deletes an outbound webhook by ID.
    pub async fn delete_outbound_webhook(&self, id: ShortId) -> Result<(), Error> {
        let deleted = self
            .store
            .delete_webhook(id)
            .await
            .map_err(|_| Error::WebhookNotFound { id })?;
        if !deleted {
            return Err(Error::WebhookNotFound { id });
        }
        Ok(())
    }

    /// Tests an outbound webhook.
    pub async fn test_outbound_webhook(
        &self,
        request: TestWebhookRequest,
    ) -> Result<TestWebhookResponse, Error> {
        let event = request.event.unwrap_or(WebhookEvent::TestPing);

        if let Some(id) = request.webhook_id {
            let stored = self
                .store
                .webhook(id)
                .await
                .map_err(|_| Error::WebhookNotFound { id })?
                .ok_or(Error::WebhookNotFound { id })?;

            r3v3rs3_api::webhook::validate_webhook_url(&stored.url)?;

            let secret = match self.store.webhook_secret(id).await {
                Ok(Some(sealed)) => {
                    let aad = webhook_aad(id);
                    self.keys()
                        .open(&aad, &sealed)
                        .ok()
                        .map(String::from_utf8)
                        .transpose()
                        .ok()
                        .flatten()
                }
                _ => None,
            };

            Ok(test_webhook_url(&stored.url, secret.as_deref(), event).await)
        } else if let Some(url) = request.url {
            r3v3rs3_api::webhook::validate_webhook_url(&url)?;
            Ok(test_webhook_url(&url, request.secret.as_deref(), event).await)
        } else {
            Err(Error::InvalidWebhook {
                reason: "either webhook_id or url is required".to_string(),
            })
        }
    }

    /// Dispatches an event to all matching outbound webhooks in the background.
    pub async fn dispatch_webhook_event(
        &self,
        event: WebhookEvent,
        summary: impl Into<String>,
        data: serde_json::Value,
    ) {
        let webhooks = match self.store.list_webhooks().await {
            Ok(list) => list,
            Err(err) => {
                warn!(%err, "failed to read webhooks for event dispatch");
                return;
            }
        };

        let matching: Vec<_> = webhooks
            .into_iter()
            .filter(|w| w.enabled && w.events.contains(&event))
            .collect();

        if matching.is_empty() {
            return;
        }

        let summary_str = summary.into();
        let payload = build_payload(event, &summary_str, data);
        let mut targets = Vec::new();

        for wh in matching {
            let secret = match self.store.webhook_secret(wh.id).await {
                Ok(Some(sealed)) => {
                    let aad = webhook_aad(wh.id);
                    self.keys().open(&aad, &sealed).ok()
                }
                _ => None,
            };
            targets.push((wh.name, wh.url, secret));
        }

        tokio::spawn(deliver_to_all(targets, payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_calculation() {
        let secret = b"super-secret-key";
        let body = b"{\"hello\":\"world\"}";
        let sig = sign_payload(secret, body);
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), 7 + 64);
    }

    #[test]
    fn payload_construction() {
        let payload = build_payload(
            WebhookEvent::DeploySuccess,
            "Deploy succeeded for web-app",
            serde_json::json!({"app_id": "123"}),
        );
        assert_eq!(payload.event, WebhookEvent::DeploySuccess);
        assert!(payload.id.starts_with("evt_"));
        assert_eq!(payload.summary, "Deploy succeeded for web-app");
        assert_eq!(payload.data["app_id"], "123");
    }
}
