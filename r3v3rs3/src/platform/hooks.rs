//! The webhook of an app: its secret, the signed push requests of the Git providers, and the
//! queue of the deployments that a push starts while the app is busy.

use super::store::WEBHOOK_SECRET;
use super::{Platform, not_found};
use anyhow::Context as _;
use hmac::{Hmac, KeyInit, Mac};
use hyper::header::HeaderMap;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, DeploymentTrigger, HookOutcome, HookResponse, WebhookSecret,
};
use serde_derive::Deserialize;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tracing::error;

/// The username of the deployments that a webhook starts.
pub const HOOK_USERNAME: &str = "webhook";

/// The Git provider of a webhook request, named by its signature header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    /// Gitea and Forgejo.
    Gitea,
    GitLab,
    /// Any sender that signs the body like GitHub does, for example a CI job.
    Generic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::Gitea => "gitea",
            Self::GitLab => "gitlab",
            Self::Generic => "generic",
        }
    }
}

/// The signature headers and the event headers of the providers, in the order of the check.
/// Gitea and Forgejo also send the GitHub header, so their own headers come first.
const SIGNATURES: [(&str, Provider, Option<&str>); 5] = [
    (
        "x-forgejo-signature",
        Provider::Gitea,
        Some("x-forgejo-event"),
    ),
    ("x-gitea-signature", Provider::Gitea, Some("x-gitea-event")),
    ("x-gitlab-token", Provider::GitLab, Some("x-gitlab-event")),
    (
        "x-hub-signature-256",
        Provider::GitHub,
        Some("x-github-event"),
    ),
    ("x-signature-256", Provider::Generic, None),
];

/// The signature of a webhook request and the event that it names.
struct Signature<'a> {
    provider: Provider,
    value: &'a str,
    event: Option<&'a str>,
}

impl Signature<'_> {
    /// The first signature header of the request.
    fn of(headers: &HeaderMap) -> Option<Signature<'_>> {
        SIGNATURES.iter().find_map(|&(name, provider, event)| {
            let value = headers.get(name)?.to_str().ok()?;
            let event = event
                .and_then(|event| headers.get(event))
                .and_then(|event| event.to_str().ok());
            Some(Signature {
                provider,
                value,
                event,
            })
        })
    }

    /// Whether the secret of the app signed the body. GitLab sends the secret itself.
    fn verify(&self, secret: &[u8], body: &[u8]) -> bool {
        match self.provider {
            Provider::GitLab => same_token(self.value.as_bytes(), secret),
            Provider::Gitea => same_mac(secret, body, self.value),
            Provider::GitHub | Provider::Generic => self
                .value
                .strip_prefix("sha256=")
                .is_some_and(|mac| same_mac(secret, body, mac)),
        }
    }

    fn is_push(&self) -> bool {
        match self.provider {
            Provider::GitHub | Provider::Gitea => self.event == Some("push"),
            Provider::GitLab => matches!(self.event, Some("Push Hook" | "Tag Push Hook")),
            Provider::Generic => true,
        }
    }
}

/// Whether `mac` is the hex HMAC-SHA256 of the body with the secret. The check takes the same
/// time for every wrong value.
fn same_mac(secret: &[u8], body: &[u8], mac: &str) -> bool {
    let Ok(expected) = hex::decode(mac) else {
        return false;
    };
    let Ok(mut hmac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    hmac.update(body);
    hmac.verify_slice(&expected).is_ok()
}

/// Compares the hashes of the values, so the check takes the same time for every length.
fn same_token(value: &[u8], secret: &[u8]) -> bool {
    Sha256::digest(value)
        .as_slice()
        .ct_eq(Sha256::digest(secret).as_slice())
        .into()
}

/// The fields of a push event that decide the deployment. GitHub, Gitea and GitLab share them.
#[derive(Deserialize)]
struct Push {
    #[serde(rename = "ref")]
    git_ref: String,
    /// GitHub and Gitea mark the push that deletes the branch.
    #[serde(default)]
    deleted: bool,
}

/// Whether the push deploys the app. A Git or Compose app deploys for a push to its branch or to
/// the tag of its branch field. An image app has no branch, so every push deploys it again. A
/// generic request carries no event, so it always deploys.
fn deploys(signature: &Signature, app: &AppEntry, body: &[u8]) -> Result<bool, Error> {
    if !signature.is_push() {
        return Ok(false);
    }
    let (AppSource::Git { branch, .. } | AppSource::Compose { branch, .. }) = &app.spec.source
    else {
        return Ok(true);
    };
    if signature.provider == Provider::Generic {
        return Ok(true);
    }
    let branch = branch.as_str();
    let push: Push = serde_json::from_slice(body).map_err(|err| Error::InvalidWebhookPayload {
        reason: err.to_string(),
    })?;
    let named = ["refs/heads/", "refs/tags/"]
        .iter()
        .any(|prefix| push.git_ref.strip_prefix(prefix) == Some(branch));
    Ok(named && !push.deleted)
}

impl Platform {
    fn queued_hooks(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<ShortId>> {
        match self.queued.lock() {
            Ok(queued) => queued,
            // The set stays valid when a holder panics.
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Creates a new webhook secret of an app, and returns the app with the secret. The old
    /// secret stops working, so the webhook that a connection installed is installed again.
    pub async fn new_webhook_secret(
        &self,
        id: ShortId,
    ) -> anyhow::Result<(AppEntry, WebhookSecret)> {
        self.app(id).await?;
        let secret = self.create_webhook_secret(id).await?;
        if let Some(hook) = self.store.app_hook(id).await? {
            self.reinstall_stored_hook(id, &hook, crate::clock::unix_ms())
                .await?;
        }
        Ok((self.app(id).await?, WebhookSecret { secret }))
    }

    /// Stores a new webhook secret of an app and returns it.
    async fn create_webhook_secret(&self, id: ShortId) -> anyhow::Result<String> {
        let secret = hex::encode(rand::random::<[u8; 32]>());
        let sealed = self.keys.seal(&webhook_secret_aad(id), secret.as_bytes())?;
        self.store.set_secret(id, WEBHOOK_SECRET, &sealed).await?;
        Ok(secret)
    }

    /// The webhook secret of an app, created when the app has none.
    pub(super) async fn ensure_webhook_secret(&self, id: ShortId) -> anyhow::Result<String> {
        match self.webhook_secret(id).await? {
            Some(secret) => String::from_utf8(secret).context("the webhook secret is not UTF-8"),
            None => self.create_webhook_secret(id).await,
        }
    }

    /// Deletes the webhook secret of an app, so that no request deploys it, and returns the app.
    /// The webhook that a connection installed is removed too.
    pub async fn delete_webhook_secret(&self, id: ShortId) -> anyhow::Result<AppEntry> {
        self.app(id).await?;
        self.drop_app_hook(id).await?;
        self.store.delete_app_hook(id).await?;
        self.store.delete_secret(id, WEBHOOK_SECRET).await?;
        self.app(id).await
    }

    async fn webhook_secret(&self, id: ShortId) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(sealed) = self.store.secret(id, WEBHOOK_SECRET).await? else {
            return Ok(None);
        };
        let secret = self
            .keys
            .open(&webhook_secret_aad(id), &sealed)
            .context("failed to open the webhook secret")?;
        Ok(Some(secret))
    }

    /// Checks the signature of a webhook request of an app, and deploys the app for a push to
    /// its branch. An app without a webhook secret answers like a missing app.
    pub async fn hook(
        self: &Arc<Self>,
        id: ShortId,
        headers: &HeaderMap,
        body: &[u8],
    ) -> anyhow::Result<(AppEntry, Provider, HookResponse)> {
        let app = self.app(id).await?;
        let secret = self
            .webhook_secret(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let signature = Signature::of(headers)
            .filter(|signature| signature.verify(&secret, body))
            .ok_or(Error::Unauthorized)?;
        let response = if deploys(&signature, &app, body)? {
            self.hook_deploy(id).await?
        } else {
            HookResponse {
                outcome: HookOutcome::Ignored,
                deployment: None,
            }
        };
        Ok((app, signature.provider, response))
    }

    /// Deploys the app, or queues one deployment while a deployment of the app runs.
    async fn hook_deploy(self: &Arc<Self>, id: ShortId) -> anyhow::Result<HookResponse> {
        match self
            .deploy(id, HOOK_USERNAME, DeploymentTrigger::Webhook)
            .await
        {
            Ok(deployment) => Ok(HookResponse {
                outcome: HookOutcome::Deployed,
                deployment: Some(deployment),
            }),
            Err(err) if matches!(err.downcast_ref(), Some(Error::AppBusy { .. })) => {
                self.queued_hooks().insert(id);
                Ok(HookResponse {
                    outcome: HookOutcome::Queued,
                    deployment: None,
                })
            }
            Err(err) => Err(err),
        }
    }

    /// Starts the deployment that a push queued while the app was busy. The pipeline calls it
    /// after it releases the app. The future is boxed, because a deployment starts the next
    /// pipeline, which calls this function again.
    pub(super) fn run_queued_hook(
        self: Arc<Self>,
        id: ShortId,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            if !self.queued_hooks().remove(&id) {
                return;
            }
            // The deployment takes the latest commit of the branch, so it covers every queued push.
            if let Err(err) = self.hook_deploy(id).await {
                error!(app = %id, "failed to start the queued webhook deployment: {err:#}");
            }
        })
    }

    /// Drops the queued deployment of a deleted app.
    pub(super) fn forget_queued_hook(&self, id: ShortId) {
        self.queued_hooks().remove(&id);
    }
}

fn webhook_secret_aad(app: ShortId) -> String {
    format!("app/{app}/secret/{WEBHOOK_SECRET}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::tests::{api_error, platform, request};
    use hyper::header::HeaderValue;
    use std::time::Duration;

    fn github_mac(secret: &str, body: &[u8]) -> String {
        let mut hmac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        hmac.update(body);
        hex::encode(hmac.finalize().into_bytes())
    }

    fn headers(pairs: &[(&'static str, String)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn github(secret: &str, event: &str, body: &[u8]) -> HeaderMap {
        headers(&[
            ("x-github-event", event.to_string()),
            (
                "x-hub-signature-256",
                format!("sha256={}", github_mac(secret, body)),
            ),
        ])
    }

    fn git_request(name: &str) -> r3v3rs3_api::platform::AppRequest {
        let mut request = request(name);
        request.spec.source = AppSource::Git {
            connection: None,
            repository: "https://example.com/shop.git".parse().unwrap(),
            branch: "main".parse().unwrap(),
            context: ".".parse().unwrap(),
            dockerfile: "Dockerfile".parse().unwrap(),
        };
        request
    }

    fn push(git_ref: &str) -> Vec<u8> {
        format!(r#"{{"ref":"{git_ref}","after":"abc"}}"#).into_bytes()
    }

    fn outcome(result: &(AppEntry, Provider, HookResponse)) -> (Provider, HookOutcome) {
        (result.1, result.2.outcome)
    }

    #[test]
    fn each_provider_signs_its_own_way() {
        let body = push("refs/heads/main");
        let mac = github_mac("s3cret", &body);
        let cases = [
            (
                headers(&[("x-hub-signature-256", format!("sha256={mac}"))]),
                Provider::GitHub,
            ),
            (
                // Gitea sends the GitHub header too, and its own header decides the provider.
                headers(&[
                    ("x-gitea-signature", mac.clone()),
                    ("x-hub-signature-256", "sha256=00".to_string()),
                ]),
                Provider::Gitea,
            ),
            (
                headers(&[("x-forgejo-signature", mac.clone())]),
                Provider::Gitea,
            ),
            (
                headers(&[("x-gitlab-token", "s3cret".to_string())]),
                Provider::GitLab,
            ),
            (
                headers(&[("x-signature-256", format!("sha256={mac}"))]),
                Provider::Generic,
            ),
        ];
        for (headers, provider) in cases {
            let signature = Signature::of(&headers).unwrap();
            assert_eq!(signature.provider, provider);
            assert!(signature.verify(b"s3cret", &body), "{provider:?}");
            assert!(!signature.verify(b"other", &body), "{provider:?}");
            // GitLab sends the secret instead of a signature of the body.
            let signs_body = provider != Provider::GitLab;
            assert_eq!(
                signature.verify(b"s3cret", b"changed"),
                !signs_body,
                "{provider:?}"
            );
        }
        // A GitHub signature needs its prefix, and a request needs a signature.
        let bare = headers(&[("x-hub-signature-256", mac)]);
        assert!(!Signature::of(&bare).unwrap().verify(b"s3cret", &body));
        assert!(Signature::of(&HeaderMap::new()).is_none());
    }

    /// Sends a request that the secret signs, and returns its error.
    async fn hook_error(platform: &Arc<Platform>, id: ShortId, secret: &str) -> Error {
        let body = push("refs/heads/main");
        let result = platform
            .hook(id, &github(secret, "push", &body), &body)
            .await;
        api_error(result.expect_err("the request must fail"))
    }

    #[tokio::test]
    async fn a_hook_needs_the_current_secret_of_the_app() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let shop = platform.add_app(git_request("shop"), 1).await?;
        let err = hook_error(&platform, shop.id, "s3cret").await;
        assert!(matches!(err, Error::IdNotFound { .. }));

        let (app, secret) = platform.new_webhook_secret(shop.id).await?;
        assert!(app.webhook_secret_set);
        assert_eq!(secret.secret.len(), 64);
        let err = hook_error(&platform, shop.id, "wrong").await;
        assert!(matches!(err, Error::Unauthorized));

        // A new secret replaces the old one, and a deleted secret deploys nothing.
        let (_, renewed) = platform.new_webhook_secret(shop.id).await?;
        assert_ne!(renewed.secret, secret.secret);
        let err = hook_error(&platform, shop.id, &secret.secret).await;
        assert!(matches!(err, Error::Unauthorized));
        let app = platform.delete_webhook_secret(shop.id).await?;
        assert!(!app.webhook_secret_set);
        let err = hook_error(&platform, shop.id, &renewed.secret).await;
        assert!(matches!(err, Error::IdNotFound { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn only_a_push_to_its_branch_deploys_a_git_app() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let shop = platform.add_app(git_request("shop"), 1).await?;
        let (_, secret) = platform.new_webhook_secret(shop.id).await?;
        let body = push("refs/heads/main");
        let signed = |event, body: &[u8]| github(&secret.secret, event, body);
        let result = platform
            .hook(shop.id, &signed("ping", b"{}"), b"{}")
            .await?;
        assert_eq!(outcome(&result), (Provider::GitHub, HookOutcome::Ignored));
        for git_ref in ["refs/heads/dev", "refs/heads/mainline", "refs/tags/v1"] {
            let other = push(git_ref);
            let result = platform
                .hook(shop.id, &signed("push", &other), &other)
                .await?;
            assert_eq!(result.2.outcome, HookOutcome::Ignored, "{git_ref}");
        }
        let deleted = br#"{"ref":"refs/heads/main","deleted":true}"#;
        let result = platform
            .hook(shop.id, &signed("push", deleted), deleted)
            .await?;
        assert_eq!(result.2.outcome, HookOutcome::Ignored);
        let err = platform
            .hook(shop.id, &signed("push", b"payload=1"), b"payload=1")
            .await
            .unwrap_err();
        assert!(matches!(
            api_error(err),
            Error::InvalidWebhookPayload { .. }
        ));

        let result = platform
            .hook(shop.id, &signed("push", &body), &body)
            .await?;
        assert_eq!(outcome(&result), (Provider::GitHub, HookOutcome::Deployed));
        let deployment = result.2.deployment.unwrap();
        assert_eq!(deployment.trigger, DeploymentTrigger::Webhook);
        assert_eq!(deployment.username, HOOK_USERNAME);
        Ok(())
    }

    #[tokio::test]
    async fn every_push_deploys_an_image_app() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let shop = platform.add_app(request("shop"), 1).await?;
        let (_, secret) = platform.new_webhook_secret(shop.id).await?;
        let body = push("refs/heads/anything");
        let headers = headers(&[
            ("x-gitlab-event", "Push Hook".to_string()),
            ("x-gitlab-token", secret.secret.clone()),
        ]);
        let result = platform.hook(shop.id, &headers, &body).await?;
        assert_eq!(outcome(&result), (Provider::GitLab, HookOutcome::Deployed));
        Ok(())
    }

    #[tokio::test]
    async fn a_push_to_a_busy_app_queues_one_more_deployment() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let shop = platform.add_app(request("shop"), 1).await?;
        let (_, secret) = platform.new_webhook_secret(shop.id).await?;
        let body = b"{}";
        let mac = github_mac(&secret.secret, body);
        let generic = headers(&[("x-signature-256", format!("sha256={mac}"))]);

        let lock = platform.lock_app(&shop)?;
        for _ in 0..2 {
            let result = platform.hook(shop.id, &generic, body).await?;
            assert_eq!(outcome(&result), (Provider::Generic, HookOutcome::Queued));
        }
        drop(lock);
        platform.clone().run_queued_hook(shop.id).await;
        // The queue holds one deployment, however many pushes arrived.
        platform.clone().run_queued_hook(shop.id).await;

        // The fake runtime fails the health check, which does not matter for the queue: the
        // deployment is finished either way.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let deployments = platform.deployments(shop.id).await?;
            if deployments.iter().all(|d| d.finished_at.is_some()) {
                assert_eq!(deployments.len(), 1);
                assert_eq!(deployments[0].trigger, DeploymentTrigger::Webhook);
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{deployments:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
}
