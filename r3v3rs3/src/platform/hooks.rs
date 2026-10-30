//! The webhook of an app: its secret, the signed push requests of the Git providers, and the
//! queue of the deployments that a push starts while the app is busy.

use super::store::WEBHOOK_SECRET;
use super::{Platform, not_found};
use anyhow::Context as _;
use hmac::{Hmac, KeyInit, Mac};
use hyper::header::HeaderMap;
use r3v3rs3_api::container::{AppName, ImageRef};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git::GitRef;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppRequest, AppSource, AppSpec, DeploymentTrigger, HookOutcome, HookResponse,
    PreviewParent, WebhookSecret,
};
use r3v3rs3_api::subject_name::SubjectName;
use serde_derive::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tracing::error;

/// The username of the deployments that a webhook starts.
pub const HOOK_USERNAME: &str = "webhook";

/// The provider or sender of a webhook request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    /// Gitea and Forgejo.
    Gitea,
    GitLab,
    DockerHub,
    Harbor,
    Quay,
    /// Any sender that signs the body like GitHub does, for example a CI job.
    Generic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::Gitea => "gitea",
            Self::GitLab => "gitlab",
            Self::DockerHub => "dockerhub",
            Self::Harbor => "harbor",
            Self::Quay => "quay",
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
    is_token: bool,
}

fn detect_registry_provider(body: &[u8]) -> Provider {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Provider::Generic;
    };
    if val.get("push_data").is_some() {
        return Provider::DockerHub;
    }
    if val.get("event_data").is_some()
        || val.get("type").and_then(|t| t.as_str()) == Some("PUSH_ARTIFACT")
    {
        return Provider::Harbor;
    }
    if val.get("updated_tags").is_some() {
        return Provider::Quay;
    }
    Provider::Generic
}

fn extract_direct_token(headers: &HeaderMap) -> Option<&str> {
    for name in ["x-webhook-token", "x-webhook-secret", "x-registry-token"] {
        if let Some(val) = headers.get(name).and_then(|v| v.to_str().ok()) {
            return Some(val);
        }
    }
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            return Some(token.trim());
        }
        if let Some(token) = auth.strip_prefix("bearer ") {
            return Some(token.trim());
        }
        return Some(auth.trim());
    }
    None
}

impl Signature<'_> {
    /// The signature or authorization token of the request.
    fn of<'a>(headers: &'a HeaderMap, body: &[u8]) -> Option<Signature<'a>> {
        for &(name, mut provider, event) in &SIGNATURES {
            let Some(val_header) = headers.get(name) else {
                continue;
            };
            let Ok(value) = val_header.to_str() else {
                continue;
            };
            let event = event
                .and_then(|ev| headers.get(ev))
                .and_then(|ev| ev.to_str().ok());
            if provider == Provider::Generic {
                let detected = detect_registry_provider(body);
                if detected != Provider::Generic {
                    provider = detected;
                }
            }
            return Some(Signature {
                provider,
                value,
                event,
                is_token: provider == Provider::GitLab,
            });
        }
        if let Some(token) = extract_direct_token(headers) {
            let provider = detect_registry_provider(body);
            return Some(Signature {
                provider,
                value: token,
                event: None,
                is_token: true,
            });
        }
        None
    }

    /// Whether the secret of the app signed the body or matches the token.
    fn verify(&self, secret: &[u8], body: &[u8]) -> bool {
        if self.is_token {
            return same_token(self.value.as_bytes(), secret);
        }
        match self.provider {
            Provider::GitLab => same_token(self.value.as_bytes(), secret),
            Provider::Gitea => same_mac(secret, body, self.value),
            _ => self
                .value
                .strip_prefix("sha256=")
                .is_some_and(|mac| same_mac(secret, body, mac)),
        }
    }

    fn is_push(&self) -> bool {
        match self.provider {
            Provider::GitHub => matches!(self.event, Some("push" | "package" | "registry_package")),
            Provider::Gitea => self.event == Some("push"),
            Provider::GitLab => matches!(self.event, Some("Push Hook" | "Tag Push Hook")),
            _ => true,
        }
    }

    fn is_pull_request(&self) -> bool {
        match self.provider {
            Provider::GitHub | Provider::Gitea => self.event == Some("pull_request"),
            Provider::GitLab => self.event == Some("Merge Request Hook"),
            _ => false,
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
    #[serde(default)]
    after: Option<String>,
}

/// What a push request should deploy.
#[derive(Debug, PartialEq, Eq)]
enum PushTarget {
    Deploy(Option<GitRef>),
    Ignore,
}

/// What a webhook request should perform.
#[derive(Debug, PartialEq, Eq)]
enum HookAction {
    Deploy(Option<GitRef>),
    PullRequest(PrPayload),
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrAction {
    Opened,
    Synchronize,
    Reopened,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrPayload {
    pub action: PrAction,
    pub number: u64,
    pub branch: String,
    pub commit: Option<String>,
}

#[derive(Deserialize)]
struct GitHubPrEvent {
    action: Option<String>,
    number: Option<u64>,
    pull_request: Option<GitHubPrData>,
}

#[derive(Deserialize)]
struct GitHubPrData {
    number: Option<u64>,
    head: Option<GitHubPrHead>,
}

#[derive(Deserialize)]
struct GitHubPrHead {
    #[serde(rename = "ref")]
    branch: Option<String>,
    sha: Option<String>,
}

#[derive(Deserialize)]
struct GitLabMrEvent {
    object_attributes: Option<GitLabMrAttributes>,
}

#[derive(Deserialize)]
struct GitLabMrAttributes {
    action: Option<String>,
    iid: Option<u64>,
    source_branch: Option<String>,
    last_commit: Option<GitLabMrCommit>,
}

#[derive(Deserialize)]
struct GitLabMrCommit {
    id: Option<String>,
}

fn parse_github_pr(body: &[u8]) -> Option<PrPayload> {
    let event: GitHubPrEvent = serde_json::from_slice(body).ok()?;
    let action_str = event.action.as_deref()?;
    let action = match action_str {
        "opened" => PrAction::Opened,
        "synchronize" => PrAction::Synchronize,
        "reopened" => PrAction::Reopened,
        "closed" => PrAction::Closed,
        _ => return None,
    };
    let number = event
        .number
        .or_else(|| event.pull_request.as_ref().and_then(|p| p.number))?;
    let head = event.pull_request.as_ref().and_then(|p| p.head.as_ref())?;
    let branch = head.branch.clone()?;
    let commit = head.sha.clone();
    Some(PrPayload {
        action,
        number,
        branch,
        commit,
    })
}

fn parse_gitlab_mr(body: &[u8]) -> Option<PrPayload> {
    let event: GitLabMrEvent = serde_json::from_slice(body).ok()?;
    let attrs = event.object_attributes.as_ref()?;
    let action_str = attrs.action.as_deref()?;
    let action = match action_str {
        "open" => PrAction::Opened,
        "update" => PrAction::Synchronize,
        "reopen" => PrAction::Reopened,
        "close" => PrAction::Closed,
        _ => return None,
    };
    let number = attrs.iid?;
    let branch = attrs.source_branch.clone()?;
    let commit = attrs.last_commit.as_ref().and_then(|c| c.id.clone());
    Some(PrPayload {
        action,
        number,
        branch,
        commit,
    })
}

fn parse_pr_payload(provider: Provider, body: &[u8]) -> Option<PrPayload> {
    match provider {
        Provider::GitHub | Provider::Gitea => parse_github_pr(body),
        Provider::GitLab => parse_gitlab_mr(body),
        _ => None,
    }
}

fn hook_action(signature: &Signature, app: &AppEntry, body: &[u8]) -> Result<HookAction, Error> {
    if signature.is_push() {
        return match push_target(signature, app, body)? {
            PushTarget::Deploy(target_ref) => Ok(HookAction::Deploy(target_ref)),
            PushTarget::Ignore => Ok(HookAction::Ignore),
        };
    }
    if signature.is_pull_request()
        && app.spec.preview.as_ref().is_some_and(|p| p.enabled)
        && let Some(pr) = parse_pr_payload(signature.provider, body)
    {
        return Ok(HookAction::PullRequest(pr));
    }
    Ok(HookAction::Ignore)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPush {
    pub repository: Option<String>,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

#[derive(Deserialize)]
struct DockerHubPayload {
    push_data: Option<DockerHubPushData>,
    repository: Option<DockerHubRepo>,
}

#[derive(Deserialize)]
struct DockerHubPushData {
    tag: Option<String>,
}

#[derive(Deserialize)]
struct DockerHubRepo {
    repo_name: Option<String>,
    name: Option<String>,
}

fn parse_dockerhub_payload(body: &[u8]) -> Option<RegistryPush> {
    let payload: DockerHubPayload = serde_json::from_slice(body).ok()?;
    let tag = payload.push_data?.tag?;
    let repo = payload.repository.and_then(|r| r.repo_name.or(r.name));
    Some(RegistryPush {
        repository: repo,
        tag: Some(tag),
        digest: None,
    })
}

#[derive(Deserialize)]
struct GitHubPackagePayload {
    action: Option<String>,
    package: Option<GitHubPackageData>,
    registry_package: Option<GitHubRegistryData>,
    repository: Option<GitHubPackageRepo>,
}

#[derive(Deserialize)]
struct GitHubPackageData {
    name: Option<String>,
    package_version: Option<GitHubPackageVersion>,
}

#[derive(Deserialize)]
struct GitHubPackageVersion {
    version: Option<String>,
    container_metadata: Option<GitHubContainerMetadata>,
}

#[derive(Deserialize)]
struct GitHubContainerMetadata {
    tag: Option<GitHubTagMetadata>,
}

#[derive(Deserialize)]
struct GitHubTagMetadata {
    name: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRegistryData {
    name: Option<String>,
    package_version: Option<GitHubRegistryVersion>,
}

#[derive(Deserialize)]
struct GitHubRegistryVersion {
    version: Option<String>,
}

#[derive(Deserialize)]
struct GitHubPackageRepo {
    full_name: Option<String>,
    name: Option<String>,
}

fn extract_gh_tag(payload: &GitHubPackagePayload) -> Option<String> {
    if let Some(pkg) = &payload.package
        && let Some(ver) = &pkg.package_version
    {
        if let Some(meta) = &ver.container_metadata
            && let Some(tag_meta) = &meta.tag
            && let Some(name) = &tag_meta.name
        {
            return Some(name.clone());
        }
        if let Some(version) = &ver.version {
            return Some(version.clone());
        }
    }
    if let Some(reg) = &payload.registry_package
        && let Some(ver) = &reg.package_version
        && let Some(version) = &ver.version
    {
        return Some(version.clone());
    }
    None
}

fn extract_gh_repo(payload: &GitHubPackagePayload) -> Option<String> {
    if let Some(repo) = &payload.repository {
        if let Some(full) = &repo.full_name {
            return Some(full.clone());
        }
        if let Some(name) = &repo.name {
            return Some(name.clone());
        }
    }
    if let Some(pkg) = &payload.package
        && let Some(name) = &pkg.name
    {
        return Some(name.clone());
    }
    if let Some(reg) = &payload.registry_package
        && let Some(name) = &reg.name
    {
        return Some(name.clone());
    }
    None
}

fn parse_github_package(body: &[u8]) -> Option<RegistryPush> {
    let payload: GitHubPackagePayload = serde_json::from_slice(body).ok()?;
    if matches!(payload.action.as_deref(), Some("deleted")) {
        return None;
    }
    let tag = extract_gh_tag(&payload);
    let repo = extract_gh_repo(&payload);
    if tag.is_none() && repo.is_none() {
        return None;
    }
    Some(RegistryPush {
        repository: repo,
        tag,
        digest: None,
    })
}

#[derive(Deserialize)]
struct HarborPayload {
    #[serde(rename = "type")]
    event_type: Option<String>,
    event_data: Option<HarborEventData>,
}

#[derive(Deserialize)]
struct HarborEventData {
    repository: Option<HarborRepo>,
    resources: Option<Vec<HarborResource>>,
}

#[derive(Deserialize)]
struct HarborRepo {
    name: Option<String>,
    repo_full_name: Option<String>,
}

#[derive(Deserialize)]
struct HarborResource {
    tag: Option<String>,
    digest: Option<String>,
}

fn parse_harbor_payload(body: &[u8]) -> Option<RegistryPush> {
    let payload: HarborPayload = serde_json::from_slice(body).ok()?;
    if matches!(payload.event_type.as_deref(), Some(t) if t != "PUSH_ARTIFACT") {
        return None;
    }
    let data = payload.event_data?;
    let resource = data.resources?.into_iter().next()?;
    let repo = data.repository.and_then(|r| r.repo_full_name.or(r.name));
    Some(RegistryPush {
        repository: repo,
        tag: resource.tag,
        digest: resource.digest,
    })
}

#[derive(Deserialize)]
struct QuayPayload {
    repository: Option<String>,
    updated_tags: Option<Vec<String>>,
}

fn parse_quay_payload(body: &[u8]) -> Option<RegistryPush> {
    let payload: QuayPayload = serde_json::from_slice(body).ok()?;
    let tag = payload.updated_tags?.into_iter().next()?;
    Some(RegistryPush {
        repository: payload.repository,
        tag: Some(tag),
        digest: None,
    })
}

#[derive(Deserialize)]
struct GenericImagePayload {
    image: Option<String>,
    repository: Option<String>,
    tag: Option<String>,
}

fn parse_generic_image_payload(body: &[u8]) -> Option<RegistryPush> {
    let payload: GenericImagePayload = serde_json::from_slice(body).ok()?;
    if let Some(image_str) = payload.image
        && let Ok(img) = image_str.parse::<ImageRef>()
    {
        return Some(RegistryPush {
            repository: Some(img.repository().to_string()),
            tag: Some(img.tag_or_digest().to_string()),
            digest: None,
        });
    }
    if payload.repository.is_some() || payload.tag.is_some() {
        return Some(RegistryPush {
            repository: payload.repository,
            tag: payload.tag,
            digest: None,
        });
    }
    None
}

fn parse_registry_push(
    provider: Provider,
    event: Option<&str>,
    body: &[u8],
) -> Option<RegistryPush> {
    if (matches!(event, Some("package" | "registry_package")) || provider == Provider::GitHub)
        && let Some(push) = parse_github_package(body)
    {
        return Some(push);
    }
    if let Some(push) = parse_dockerhub_payload(body) {
        return Some(push);
    }
    if let Some(push) = parse_harbor_payload(body) {
        return Some(push);
    }
    if let Some(push) = parse_quay_payload(body) {
        return Some(push);
    }
    parse_generic_image_payload(body)
}

fn matches_repository(app_repo: &str, payload_repo: &str) -> bool {
    if app_repo == payload_repo {
        return true;
    }
    let app_name = app_repo.rsplit_once('/').map_or(app_repo, |(_, r)| r);
    let payload_name = payload_repo
        .rsplit_once('/')
        .map_or(payload_repo, |(_, r)| r);
    if app_name != payload_name {
        return false;
    }
    app_repo.ends_with(payload_repo) || payload_repo.ends_with(app_repo)
}

fn check_image_tag(
    app: &AppEntry,
    image: &ImageRef,
    tag: Option<&str>,
) -> Result<PushTarget, Error> {
    let Some(tag) = tag else {
        return Ok(PushTarget::Deploy(None));
    };
    if let Some(pattern) = app.spec.tag_trigger.as_deref()
        && r3v3rs3_api::platform::matches_tag_pattern(pattern, tag)
    {
        let target = tag.parse().map_err(|_| Error::InvalidWebhookPayload {
            reason: format!("invalid tag name: {tag}"),
        })?;
        return Ok(PushTarget::Deploy(Some(target)));
    }
    if tag == image.tag_or_digest() {
        return Ok(PushTarget::Deploy(None));
    }
    Ok(PushTarget::Ignore)
}

fn image_push_target(
    app: &AppEntry,
    image: &ImageRef,
    push: Option<RegistryPush>,
) -> Result<PushTarget, Error> {
    let Some(push) = push else {
        return Ok(PushTarget::Deploy(None));
    };
    if let Some(ref repo) = push.repository
        && !matches_repository(image.repository(), repo)
    {
        return Ok(PushTarget::Ignore);
    }
    check_image_tag(app, image, push.tag.as_deref())
}

/// Whether the push deploys the app, and with which target ref (if specified).
fn push_target(signature: &Signature, app: &AppEntry, body: &[u8]) -> Result<PushTarget, Error> {
    if !signature.is_push() {
        return Ok(PushTarget::Ignore);
    }
    match &app.spec.source {
        AppSource::Image { image } => {
            let registry_push = parse_registry_push(signature.provider, signature.event, body);
            image_push_target(app, image, registry_push)
        }
        AppSource::Git { branch, .. } | AppSource::Compose { branch, .. } => {
            if signature.provider == Provider::Generic {
                return Ok(PushTarget::Deploy(None));
            }
            let push: Push =
                serde_json::from_slice(body).map_err(|err| Error::InvalidWebhookPayload {
                    reason: err.to_string(),
                })?;
            if push.deleted || is_empty_commit(push.after.as_deref()) {
                return Ok(PushTarget::Ignore);
            }
            check_push_ref(
                branch.as_str(),
                app.spec.tag_trigger.as_deref(),
                &push.git_ref,
            )
        }
    }
}

fn is_empty_commit(after: Option<&str>) -> bool {
    after == Some("0000000000000000000000000000000000000000")
}

fn check_push_ref(
    branch: &str,
    tag_trigger: Option<&str>,
    git_ref: &str,
) -> Result<PushTarget, Error> {
    if let Some(tag) = git_ref.strip_prefix("refs/tags/") {
        if let Some(pattern) = tag_trigger
            && r3v3rs3_api::platform::matches_tag_pattern(pattern, tag)
        {
            let target = tag.parse().map_err(|_| Error::InvalidWebhookPayload {
                reason: format!("invalid tag name: {tag}"),
            })?;
            return Ok(PushTarget::Deploy(Some(target)));
        }
        if tag == branch {
            return Ok(PushTarget::Deploy(None));
        }
        return Ok(PushTarget::Ignore);
    }
    if let Some(head) = git_ref.strip_prefix("refs/heads/")
        && head == branch
    {
        return Ok(PushTarget::Deploy(None));
    }
    Ok(PushTarget::Ignore)
}

fn preview_app_name(parent_name: &str, pr_number: u64) -> Result<AppName, Error> {
    let suffix = format!("-pr-{pr_number}");
    let max_base_len = 63_usize.saturating_sub(suffix.len());
    let base = if parent_name.len() > max_base_len {
        &parent_name[..max_base_len]
    } else {
        parent_name
    };
    let base = base.trim_end_matches('-');
    let name_str = format!("{base}{suffix}");
    name_str.parse::<AppName>()
}

fn derive_preview_domain(parent: &AppEntry, pr_number: u64) -> Result<SubjectName, Error> {
    if let Some(template) = parent
        .spec
        .preview
        .as_ref()
        .and_then(|p| p.domain_template.as_deref())
        .filter(|t| !t.is_empty())
    {
        let domain_str = template
            .replace("{pr}", &pr_number.to_string())
            .replace("{app}", parent.name.as_str());
        return domain_str
            .parse::<SubjectName>()
            .map_err(|e| Error::InvalidContainerSpec {
                reason: format!("invalid preview domain template: {e}"),
            });
    }

    if let Some(SubjectName::DnsName(domain)) = parent.spec.domains.first() {
        let domain_str = format!("pr-{pr_number}.{domain}");
        return domain_str
            .parse::<SubjectName>()
            .map_err(|e| Error::InvalidContainerSpec {
                reason: format!("invalid preview domain: {e}"),
            });
    }

    let domain_str = format!("pr-{pr_number}-{}.localhost", parent.name.as_str());
    domain_str
        .parse::<SubjectName>()
        .map_err(|e| Error::InvalidContainerSpec {
            reason: format!("invalid preview domain: {e}"),
        })
}

fn preview_app_spec(parent: &AppEntry, pr: &PrPayload, domain: SubjectName) -> AppSpec {
    let mut spec = parent.spec.clone();
    if let Ok(branch_ref) = pr.branch.parse::<GitRef>() {
        match &mut spec.source {
            AppSource::Git { branch, .. } | AppSource::Compose { branch, .. } => {
                *branch = branch_ref;
            }
            AppSource::Image { .. } => {}
        }
    }
    spec.domains = vec![domain];
    spec.host_port = None;
    spec.tag_trigger = None;
    spec.auto_deploy = false;
    spec.preview = None;
    spec.environment_id = None;
    spec.env_groups = Vec::new();
    spec.preview_of = Some(PreviewParent {
        parent_id: parent.id,
        pr_number: pr.number,
    });
    spec
}

impl Platform {
    fn queued_hooks(&self) -> std::sync::MutexGuard<'_, HashMap<ShortId, Option<GitRef>>> {
        match self.queued.lock() {
            Ok(queued) => queued,
            // The map stays valid when a holder panics.
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
        let sealed = self
            .keys()
            .seal(&webhook_secret_aad(id), secret.as_bytes())?;
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
            .keys()
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
        let signature = Signature::of(headers, body)
            .filter(|signature| signature.verify(&secret, body))
            .ok_or(Error::Unauthorized)?;
        let action = hook_action(&signature, &app, body)?;
        let response = match action {
            HookAction::Deploy(target_ref) => self.hook_deploy(id, target_ref).await?,
            HookAction::PullRequest(pr) => self.handle_pull_request(&app, pr).await?,
            HookAction::Ignore => HookResponse {
                outcome: HookOutcome::Ignored,
                deployment: None,
            },
        };
        Ok((app, signature.provider, response))
    }

    async fn find_preview_app(
        &self,
        parent_id: ShortId,
        pr_number: u64,
    ) -> anyhow::Result<Option<AppEntry>> {
        let apps = self.apps().await?;
        Ok(apps.into_iter().find(|app| {
            app.spec
                .preview_of
                .as_ref()
                .is_some_and(|p| p.parent_id == parent_id && p.pr_number == pr_number)
        }))
    }

    async fn create_preview_app(
        self: &Arc<Self>,
        parent: &AppEntry,
        pr: &PrPayload,
    ) -> anyhow::Result<AppEntry> {
        let name = preview_app_name(parent.name.as_str(), pr.number)?;
        let domain = derive_preview_domain(parent, pr.number)?;
        let spec = preview_app_spec(parent, pr, domain);
        let request = AppRequest {
            name,
            target: parent.target,
            spec,
        };
        let preview_app = self.add_app(request, crate::clock::unix_ms()).await?;
        self.copy_non_secret_app_env(parent.id, preview_app.id).await?;
        Ok(preview_app)
    }

    async fn update_preview_app(
        &self,
        existing: &AppEntry,
        pr: &PrPayload,
    ) -> anyhow::Result<AppEntry> {
        let mut request = AppRequest {
            name: existing.name.clone(),
            target: existing.target,
            spec: existing.spec.clone(),
        };
        if let Ok(branch_ref) = pr.branch.parse::<GitRef>() {
            match &mut request.spec.source {
                AppSource::Git { branch, .. } | AppSource::Compose { branch, .. } => {
                    *branch = branch_ref;
                }
                AppSource::Image { .. } => {}
            }
        }
        self.update_app(existing.id, request, crate::clock::unix_ms())
            .await
    }

    async fn handle_pull_request(
        self: &Arc<Self>,
        parent: &AppEntry,
        pr: PrPayload,
    ) -> anyhow::Result<HookResponse> {
        match pr.action {
            PrAction::Opened | PrAction::Reopened | PrAction::Synchronize => {
                self.deploy_preview(parent, pr).await
            }
            PrAction::Closed => self.close_preview(parent, pr.number).await,
        }
    }

    async fn deploy_preview(
        self: &Arc<Self>,
        parent: &AppEntry,
        pr: PrPayload,
    ) -> anyhow::Result<HookResponse> {
        let target_app = match self.find_preview_app(parent.id, pr.number).await? {
            Some(existing) => self.update_preview_app(&existing, &pr).await?,
            None => self.create_preview_app(parent, &pr).await?,
        };
        let target_ref = pr.commit.as_deref().and_then(|c| c.parse().ok());
        let resp = self.hook_deploy(target_app.id, target_ref).await?;
        let outcome = match resp.outcome {
            HookOutcome::Deployed => HookOutcome::PreviewDeployed,
            other => other,
        };
        Ok(HookResponse {
            outcome,
            deployment: resp.deployment,
        })
    }

    async fn close_preview(
        self: &Arc<Self>,
        parent: &AppEntry,
        pr_number: u64,
    ) -> anyhow::Result<HookResponse> {
        let Some(existing) = self.find_preview_app(parent.id, pr_number).await? else {
            return Ok(HookResponse {
                outcome: HookOutcome::Ignored,
                deployment: None,
            });
        };
        for _ in 0..30 {
            match self.delete_app(existing.id).await {
                Ok(_) => {
                    return Ok(HookResponse {
                        outcome: HookOutcome::PreviewDestroyed,
                        deployment: None,
                    });
                }
                Err(err) if matches!(err.downcast_ref(), Some(Error::AppBusy { .. })) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(err) => return Err(err),
            }
        }
        self.delete_app(existing.id).await?;
        Ok(HookResponse {
            outcome: HookOutcome::PreviewDestroyed,
            deployment: None,
        })
    }

    /// Deploys the app, or queues one deployment while a deployment of the app runs.
    async fn hook_deploy(
        self: &Arc<Self>,
        id: ShortId,
        target_ref: Option<GitRef>,
    ) -> anyhow::Result<HookResponse> {
        match self
            .deploy_with_ref(
                id,
                HOOK_USERNAME,
                DeploymentTrigger::Webhook,
                target_ref.clone(),
            )
            .await
        {
            Ok(deployment) => Ok(HookResponse {
                outcome: HookOutcome::Deployed,
                deployment: Some(deployment),
            }),
            Err(err) if matches!(err.downcast_ref(), Some(Error::AppBusy { .. })) => {
                self.queued_hooks().insert(id, target_ref);
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
            let Some(target_ref) = self.queued_hooks().remove(&id) else {
                return;
            };
            // The deployment takes the latest commit of the branch or tag, so it covers every queued push.
            if let Err(err) = self.hook_deploy(id, target_ref).await {
                error!(app = %id, "failed to start the queued webhook deployment: {err:#}");
            }
        })
    }

    /// Drops the queued deployment of a deleted app.
    pub(super) fn forget_queued_hook(&self, id: ShortId) {
        self.queued_hooks().remove(&id);
    }
}

pub(super) fn webhook_secret_aad(app: ShortId) -> String {
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
            let signature = Signature::of(&headers, &body).unwrap();
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
        assert!(
            !Signature::of(&bare, &body)
                .unwrap()
                .verify(b"s3cret", &body)
        );
        assert!(Signature::of(&HeaderMap::new(), &body).is_none());
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

    #[tokio::test]
    async fn tag_trigger_deploys_matching_tags_on_git_app() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let mut req = git_request("tag-app");
        req.spec.tag_trigger = Some("v*".to_string());
        let app = platform.add_app(req, 1).await?;
        let (_, secret) = platform.new_webhook_secret(app.id).await?;
        let signed = |event, body: &[u8]| github(&secret.secret, event, body);

        // Non-matching tag is ignored
        let other_tag = push("refs/tags/beta-1");
        let result = platform
            .hook(app.id, &signed("push", &other_tag), &other_tag)
            .await?;
        assert_eq!(outcome(&result), (Provider::GitHub, HookOutcome::Ignored));

        // Deleted matching tag is ignored
        let deleted_tag = br#"{"ref":"refs/tags/v1.0.1","deleted":true}"#;
        let result = platform
            .hook(app.id, &signed("push", deleted_tag), deleted_tag)
            .await?;
        assert_eq!(outcome(&result), (Provider::GitHub, HookOutcome::Ignored));

        // Matching tag push deploys
        let tag_body = push("refs/tags/v1.0.0");
        let result = platform
            .hook(app.id, &signed("push", &tag_body), &tag_body)
            .await?;
        assert_eq!(outcome(&result), (Provider::GitHub, HookOutcome::Deployed));
        let deployment = result.2.deployment.unwrap();
        assert_eq!(deployment.trigger, DeploymentTrigger::Webhook);

        // Verify deployment snapshot recorded the tag as the branch
        let (snap_spec, _) = platform
            .store
            .deployment_snapshot(deployment.id)
            .await?
            .unwrap();
        if let AppSource::Git { branch, .. } = snap_spec.source {
            assert_eq!(branch.as_str(), "v1.0.0");
        } else {
            panic!("expected Git source");
        }

        // Another push while app is busy gets queued
        let branch_body = push("refs/heads/main");
        let result = platform
            .hook(app.id, &signed("push", &branch_body), &branch_body)
            .await?;
        assert_eq!(outcome(&result), (Provider::GitHub, HookOutcome::Queued));
        Ok(())
    }

    #[test]
    fn preview_app_name_and_domain_generation() {
        let name = preview_app_name("my-service", 42).unwrap();
        assert_eq!(name.as_str(), "my-service-pr-42");

        let long = "a".repeat(60);
        let long_name = preview_app_name(&long, 1234).unwrap();
        assert!(long_name.as_str().len() <= 63);
        assert!(long_name.as_str().ends_with("-pr-1234"));

        let mut parent_req = git_request("parent-app");
        parent_req.spec.domains = vec!["app.example.com".parse().unwrap()];
        let parent = AppEntry {
            id: "p1".parse().unwrap(),
            name: parent_req.name,
            target: parent_req.target,
            spec: parent_req.spec,
            git_token_set: false,
            webhook_secret_set: false,
            hook: None,
            update: None,
            catalog_update: None,
            created_at: 0,
            updated_at: 0,
        };

        // Derives from parent's primary domain
        let domain = derive_preview_domain(&parent, 42).unwrap();
        assert_eq!(domain.to_string(), "pr-42.app.example.com");

        // Custom template
        let mut custom_parent = parent.clone();
        custom_parent.spec.preview = Some(r3v3rs3_api::platform::PreviewConfig {
            enabled: true,
            domain_template: Some("preview-{pr}-{app}.test.org".to_string()),
        });
        let custom_domain = derive_preview_domain(&custom_parent, 99).unwrap();
        assert_eq!(custom_domain.to_string(), "preview-99-parent-app.test.org");
    }

    async fn wait_deployments(platform: &Platform, id: ShortId) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let deployments = platform.deployments(id).await?;
            if !deployments.is_empty()
                && deployments.iter().all(|d| d.finished_at.is_some())
                && !platform.busy_apps().contains(&id)
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for deployments of {id}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn setup_parent_app_with_secrets(
        platform: &Platform,
    ) -> anyhow::Result<(AppEntry, r3v3rs3_api::platform::WebhookSecret)> {
        let mut req = git_request("main-app");
        req.spec.preview = Some(r3v3rs3_api::platform::PreviewConfig {
            enabled: true,
            domain_template: None,
        });
        let parent = platform.add_app(req, 1).await?;
        let env_entries = vec![
            r3v3rs3_api::platform::EnvEntry {
                key: "PUBLIC_CONFIG".parse().expect("valid key"),
                value: Some("public".to_string()),
                secret: false,
            },
            r3v3rs3_api::platform::EnvEntry {
                key: "API_SECRET".parse().expect("valid key"),
                value: Some("secret123".to_string()),
                secret: true,
            },
        ];
        platform.set_env(parent.id, env_entries).await?;
        platform
            .set_git_token(parent.id, "private-repo-token")
            .await?;
        let (_, secret) = platform.new_webhook_secret(parent.id).await?;
        Ok((parent, secret))
    }

    async fn assert_preview_secrets_isolated(
        platform: &Platform,
        preview: &AppEntry,
    ) -> anyhow::Result<()> {
        let preview_env = platform.env(preview.id).await?;
        assert_eq!(preview_env.len(), 1);
        assert_eq!(preview_env[0].key.as_str(), "PUBLIC_CONFIG");
        assert!(!preview_env[0].secret);
        assert!(platform.git_token(preview.id).await?.is_none());
        assert!(preview.spec.environment_id.is_none());
        assert!(preview.spec.env_groups.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn github_pull_request_open_and_sync() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let (parent, secret) = setup_parent_app_with_secrets(&platform).await?;
        let signed = |event, body: &[u8]| github(&secret.secret, event, body);

        // PR Opened
        let pr_open_body = br#"{"action":"opened","number":42,"pull_request":{"head":{"ref":"feature-pr","sha":"commit1"}}}"#;
        let res = platform
            .hook(
                parent.id,
                &signed("pull_request", pr_open_body),
                pr_open_body,
            )
            .await?;
        assert_eq!(
            outcome(&res),
            (Provider::GitHub, HookOutcome::PreviewDeployed)
        );

        let preview = platform
            .find_preview_app(parent.id, 42)
            .await?
            .expect("preview app must exist");
        assert_eq!(preview.name.as_str(), "main-app-pr-42");
        assert_preview_secrets_isolated(&platform, &preview).await?;
        wait_deployments(&platform, preview.id).await?;

        // PR Synchronize
        let pr_sync_body = br#"{"action":"synchronize","number":42,"pull_request":{"head":{"ref":"feature-pr","sha":"commit2"}}}"#;
        let res = platform
            .hook(
                parent.id,
                &signed("pull_request", pr_sync_body),
                pr_sync_body,
            )
            .await?;
        assert_eq!(
            outcome(&res),
            (Provider::GitHub, HookOutcome::PreviewDeployed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn github_pull_request_close() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let mut req = git_request("main-app");
        req.spec.preview = Some(r3v3rs3_api::platform::PreviewConfig {
            enabled: true,
            domain_template: None,
        });
        let parent = platform.add_app(req, 1).await?;
        let (_, secret) = platform.new_webhook_secret(parent.id).await?;
        let signed = |event, body: &[u8]| github(&secret.secret, event, body);

        let pr_open = br#"{"action":"opened","number":42,"pull_request":{"head":{"ref":"feat"}}}"#;
        platform
            .hook(parent.id, &signed("pull_request", pr_open), pr_open)
            .await?;
        let preview = platform.find_preview_app(parent.id, 42).await?.unwrap();
        wait_deployments(&platform, preview.id).await?;

        let pr_close = br#"{"action":"closed","number":42,"pull_request":{"head":{"ref":"feat"}}}"#;
        let res = platform
            .hook(parent.id, &signed("pull_request", pr_close), pr_close)
            .await?;
        assert_eq!(
            outcome(&res),
            (Provider::GitHub, HookOutcome::PreviewDestroyed)
        );
        assert!(platform.find_preview_app(parent.id, 42).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn gitlab_merge_request_lifecycle() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let mut req = git_request("gitlab-app");
        req.spec.preview = Some(r3v3rs3_api::platform::PreviewConfig {
            enabled: true,
            domain_template: None,
        });
        let parent = platform.add_app(req, 1).await?;
        let (_, secret) = platform.new_webhook_secret(parent.id).await?;

        let mr_open =
            br#"{"object_attributes":{"action":"open","iid":7,"source_branch":"feature-gl"}}"#;
        let headers = headers(&[
            ("x-gitlab-event", "Merge Request Hook".to_string()),
            ("x-gitlab-token", secret.secret.clone()),
        ]);
        let res = platform.hook(parent.id, &headers, mr_open).await?;
        assert_eq!(
            outcome(&res),
            (Provider::GitLab, HookOutcome::PreviewDeployed)
        );

        let preview = platform
            .find_preview_app(parent.id, 7)
            .await?
            .expect("preview app exists");
        assert_eq!(preview.name.as_str(), "gitlab-app-pr-7");
        wait_deployments(&platform, preview.id).await?;

        let mr_close =
            br#"{"object_attributes":{"action":"close","iid":7,"source_branch":"feature-gl"}}"#;
        let res = platform.hook(parent.id, &headers, mr_close).await?;
        assert_eq!(
            outcome(&res),
            (Provider::GitLab, HookOutcome::PreviewDestroyed)
        );
        assert!(platform.find_preview_app(parent.id, 7).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn docker_hub_webhook_deploys_image_app() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let mut req = request("hub-app");
        req.spec.source = AppSource::Image {
            image: "myorg/backend:latest".parse().unwrap(),
        };
        let app = platform.add_app(req, 1).await?;
        let (_, secret) = platform.new_webhook_secret(app.id).await?;

        // 1. Matching tag and repository -> Deployed
        let body = br#"{"push_data":{"tag":"latest"},"repository":{"repo_name":"myorg/backend"}}"#;
        let headers = headers(&[("x-webhook-token", secret.secret.clone())]);
        let res = platform.hook(app.id, &headers, body).await?;
        assert_eq!(outcome(&res), (Provider::DockerHub, HookOutcome::Deployed));

        // 2. Mismatched repository -> Ignored
        let other_repo =
            br#"{"push_data":{"tag":"latest"},"repository":{"repo_name":"otherorg/other"}}"#;
        let res = platform.hook(app.id, &headers, other_repo).await?;
        assert_eq!(outcome(&res), (Provider::DockerHub, HookOutcome::Ignored));

        // 3. Mismatched tag -> Ignored
        let other_tag =
            br#"{"push_data":{"tag":"v2-dev"},"repository":{"repo_name":"myorg/backend"}}"#;
        let res = platform.hook(app.id, &headers, other_tag).await?;
        assert_eq!(outcome(&res), (Provider::DockerHub, HookOutcome::Ignored));
        Ok(())
    }

    #[tokio::test]
    async fn ghcr_package_webhook_updates_tag_trigger() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let mut req = request("ghcr-app");
        req.spec.source = AppSource::Image {
            image: "ghcr.io/org/service:v1.0".parse().unwrap(),
        };
        req.spec.tag_trigger = Some("v*".to_string());
        let app = platform.add_app(req, 1).await?;
        let (_, secret) = platform.new_webhook_secret(app.id).await?;

        let body = br#"{
            "action": "published",
            "package": {
                "name": "service",
                "package_version": {
                    "version": "v1.2",
                    "container_metadata": {
                        "tag": { "name": "v1.2" }
                    }
                }
            },
            "repository": { "full_name": "org/service" }
        }"#;
        let mac = github_mac(&secret.secret, body);
        let headers = headers(&[
            ("x-github-event", "package".to_string()),
            ("x-hub-signature-256", format!("sha256={mac}")),
        ]);
        let res = platform.hook(app.id, &headers, body).await?;
        assert_eq!(outcome(&res), (Provider::GitHub, HookOutcome::Deployed));

        let updated = platform.app(app.id).await?;
        let AppSource::Image { image } = updated.spec.source else {
            panic!("expected image source");
        };
        assert_eq!(image.as_str(), "ghcr.io/org/service:v1.2");
        Ok(())
    }

    #[tokio::test]
    async fn harbor_and_quay_webhooks_deploy() -> anyhow::Result<()> {
        let (platform, _dir) = platform().await?;
        let mut req = request("registry-app");
        req.spec.source = AppSource::Image {
            image: "harbor.domain/library/app:1.25".parse().unwrap(),
        };
        let app = platform.add_app(req, 1).await?;
        let (_, secret) = platform.new_webhook_secret(app.id).await?;

        // Harbor with Authorization: Bearer
        let harbor_body = br#"{
            "type": "PUSH_ARTIFACT",
            "event_data": {
                "repository": { "repo_full_name": "library/app" },
                "resources": [{ "tag": "1.25", "digest": "sha256:abc" }]
            }
        }"#;
        let harbor_headers = headers(&[("authorization", format!("Bearer {}", secret.secret))]);
        let res = platform.hook(app.id, &harbor_headers, harbor_body).await?;
        assert_eq!(outcome(&res), (Provider::Harbor, HookOutcome::Deployed));
        wait_deployments(&platform, app.id).await?;

        // Quay with x-webhook-secret
        let quay_body = br#"{"repository":"library/app","updated_tags":["1.25"]}"#;
        let quay_headers = headers(&[("x-webhook-secret", secret.secret.clone())]);
        let res = platform.hook(app.id, &quay_headers, quay_body).await?;
        assert_eq!(outcome(&res), (Provider::Quay, HookOutcome::Deployed));
        Ok(())
    }
}
