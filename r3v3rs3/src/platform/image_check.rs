//! Checks Docker/OCI registries for new tags and updated digests of app images.

use crate::cdn::fetch::{HttpClient, REQUEST_TIMEOUT, build_client};
use crate::clock::unix_ms;
use crate::platform::Platform;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request,
    header::{ACCEPT, AUTHORIZATION, USER_AGENT, WWW_AUTHENTICATE},
};
use r3v3rs3_api::container::ImageRef;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppRequest, AppSource, DeploymentTrigger, ImageUpdateStatus,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// How often the background checker checks all image apps.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

const MAX_REGISTRY_BODY_SIZE: usize = 2 * 1024 * 1024;
const MANIFEST_ACCEPTS: &str = "application/vnd.docker.distribution.manifest.v2+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/vnd.oci.image.manifest.v1+json, \
    application/vnd.oci.image.index.v1+json";

/// Parses an image reference into registry host, repository and tag.
pub fn parse_image_target(image: &ImageRef) -> (String, String, String) {
    let repo_raw = image.repository();
    let tag = image.tag_or_digest().to_string();

    let (host, repo) = match repo_raw.split_once('/') {
        Some((domain, rest))
            if domain.contains('.') || domain.contains(':') || domain == "localhost" =>
        {
            if domain == "docker.io" || domain == "index.docker.io" {
                if rest.contains('/') {
                    ("registry-1.docker.io".to_string(), rest.to_string())
                } else {
                    (
                        "registry-1.docker.io".to_string(),
                        format!("library/{rest}"),
                    )
                }
            } else {
                (domain.to_string(), rest.to_string())
            }
        }
        Some(_) => ("registry-1.docker.io".to_string(), repo_raw.to_string()),
        None => (
            "registry-1.docker.io".to_string(),
            format!("library/{repo_raw}"),
        ),
    };
    (host, repo, tag)
}

/// Parses the WWW-Authenticate header challenge from a 401 response.
fn parse_auth_challenge(header_val: &str) -> Option<(String, Option<String>, Option<String>)> {
    let rest = header_val.strip_prefix("Bearer ")?;
    let mut realm = None;
    let mut service = None;
    let mut scope = None;

    for part in rest.split(',') {
        let (k, v) = part.trim().split_once('=')?;
        let v = v.trim_matches('"');
        match k.trim() {
            "realm" => realm = Some(v.to_string()),
            "service" => service = Some(v.to_string()),
            "scope" => scope = Some(v.to_string()),
            _ => {}
        }
    }
    realm.map(|r| (r, service, scope))
}

fn build_token_url(realm: &str, service: Option<&str>, scope: Option<&str>) -> String {
    let mut url = format!("{realm}?");
    if let Some(service) = service {
        url.push_str(&format!("service={service}&"));
    }
    if let Some(scope) = scope {
        url.push_str(&format!("scope={scope}&"));
    }
    url
}

async fn execute_request(
    client: &HttpClient,
    request: Request<Full<Bytes>>,
) -> anyhow::Result<(hyper::StatusCode, hyper::HeaderMap, Bytes)> {
    let response = tokio::time::timeout(REQUEST_TIMEOUT, client.request(request)).await??;
    let status = response.status();
    let headers = response.headers().clone();
    let body = tokio::time::timeout(
        REQUEST_TIMEOUT,
        Limited::new(response.into_body(), MAX_REGISTRY_BODY_SIZE).collect(),
    )
    .await?
    .map_err(|err| anyhow::anyhow!(err))?
    .to_bytes();
    Ok((status, headers, body))
}

fn build_http_request(
    url: &str,
    method: Method,
    accept: Option<&str>,
    token: Option<&str>,
) -> anyhow::Result<Request<Full<Bytes>>> {
    let mut builder = Request::builder()
        .uri(url)
        .method(method)
        .header(USER_AGENT, concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")));
    if let Some(accept) = accept {
        builder = builder.header(ACCEPT, accept);
    }
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    Ok(builder.body(Full::new(Bytes::new()))?)
}

async fn get_bearer_token(
    client: &HttpClient,
    realm: &str,
    service: Option<&str>,
    scope: Option<&str>,
) -> anyhow::Result<String> {
    let url = build_token_url(realm, service, scope);
    let req = Request::builder()
        .uri(url)
        .header(USER_AGENT, concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")))
        .body(Full::new(Bytes::new()))?;
    let (status, _, body) = execute_request(client, req).await?;
    if !status.is_success() {
        anyhow::bail!("failed to authenticate with registry: status {status}");
    }
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    let token = json
        .get("token")
        .or_else(|| json.get("access_token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("token missing in auth response"))?;
    Ok(token.to_string())
}

async fn fetch_with_auth(
    client: &HttpClient,
    url: &str,
    method: Method,
    accept: Option<&str>,
) -> anyhow::Result<(hyper::StatusCode, hyper::HeaderMap, Bytes)> {
    let req = build_http_request(url, method.clone(), accept, None)?;
    let (status, headers, body) = execute_request(client, req).await?;
    if status != hyper::StatusCode::UNAUTHORIZED {
        return Ok((status, headers, body));
    }
    let Some(auth_header) = headers.get(WWW_AUTHENTICATE) else {
        return Ok((status, headers, body));
    };
    let auth_str = auth_header.to_str()?;
    let Some((realm, service, scope)) = parse_auth_challenge(auth_str) else {
        return Ok((status, headers, body));
    };
    let token = get_bearer_token(client, &realm, service.as_deref(), scope.as_deref()).await?;
    let req = build_http_request(url, method, accept, Some(&token))?;
    execute_request(client, req).await
}

async fn fetch_manifest_digest(
    client: &HttpClient,
    host: &str,
    repo: &str,
    tag: &str,
) -> anyhow::Result<String> {
    let url = format!("https://{host}/v2/{repo}/manifests/{tag}");
    let (status, headers, body) =
        fetch_with_auth(client, &url, Method::GET, Some(MANIFEST_ACCEPTS)).await?;
    if !status.is_success() {
        anyhow::bail!("registry returned {status} for manifest {repo}:{tag}");
    }
    if let Some(digest) = headers.get("docker-content-digest") {
        return Ok(digest.to_str()?.to_string());
    }
    use sha2::Digest;
    let digest_bytes = sha2::Sha256::digest(&body);
    let hash = format!("sha256:{}", hex::encode(digest_bytes));
    Ok(hash)
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    tags: Vec<String>,
}

async fn fetch_tags(client: &HttpClient, host: &str, repo: &str) -> anyhow::Result<Vec<String>> {
    let url = format!("https://{host}/v2/{repo}/tags/list");
    let (status, _, body) = fetch_with_auth(client, &url, Method::GET, None).await?;
    if !status.is_success() {
        anyhow::bail!("registry returned {status} for tags of {repo}");
    }
    let parsed: TagsResponse = serde_json::from_slice(&body)?;
    Ok(parsed.tags)
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedTag<'a> {
    numbers: Vec<u64>,
    suffix: &'a str,
}

fn parse_tag(tag: &str) -> Option<ParsedTag<'_>> {
    let tag_no_v = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    let (version_part, suffix) = match tag_no_v.find('-') {
        Some(idx) => (&tag_no_v[..idx], &tag_no_v[idx..]),
        None => (tag_no_v, ""),
    };
    let mut numbers = Vec::new();
    for part in version_part.split('.') {
        if part.is_empty() {
            return None;
        }
        let num = part.parse::<u64>().ok()?;
        numbers.push(num);
    }
    if numbers.is_empty() {
        return None;
    }
    Some(ParsedTag { numbers, suffix })
}

pub fn find_newer_tag(current_tag: &str, tags: &[String]) -> Option<String> {
    let current = parse_tag(current_tag)?;
    let mut best: Option<(Vec<u64>, &String)> = None;

    for tag in tags {
        let Some(parsed) = parse_tag(tag) else {
            continue;
        };
        if parsed.suffix != current.suffix {
            continue;
        }
        if parsed.numbers > current.numbers {
            let is_better = best.as_ref().is_none_or(|(nums, _)| parsed.numbers > *nums);
            if is_better {
                best = Some((parsed.numbers, tag));
            }
        }
    }

    best.map(|(_, tag)| tag.clone())
}

pub fn hash_of_digest(digest: &str) -> &str {
    if let Some((_, hash)) = digest.split_once('@') {
        hash
    } else {
        digest
    }
}

pub async fn check_image_update(
    image: &ImageRef,
    running_digest: Option<&str>,
) -> anyhow::Result<ImageUpdateStatus> {
    let (host, repo, tag) = parse_image_target(image);
    let now = unix_ms();
    if tag.starts_with("sha256:") {
        return Ok(ImageUpdateStatus {
            update_available: false,
            current_tag: Some(tag),
            latest_tag: None,
            current_digest: running_digest.map(str::to_string),
            latest_digest: None,
            checked_at: now,
        });
    }

    let client = build_client().await?;
    let tags = fetch_tags(&client, &host, &repo).await.unwrap_or_default();
    let newer_tag = find_newer_tag(&tag, &tags);

    if let Some(newer) = newer_tag {
        let latest_digest = fetch_manifest_digest(&client, &host, &repo, &newer)
            .await
            .ok();
        return Ok(ImageUpdateStatus {
            update_available: true,
            current_tag: Some(tag),
            latest_tag: Some(newer),
            current_digest: running_digest.map(str::to_string),
            latest_digest,
            checked_at: now,
        });
    }

    let remote_digest = fetch_manifest_digest(&client, &host, &repo, &tag).await?;
    let update_available = match running_digest {
        Some(deployed) => hash_of_digest(deployed) != remote_digest,
        None => false,
    };

    Ok(ImageUpdateStatus {
        update_available,
        current_tag: Some(tag),
        latest_tag: None,
        current_digest: running_digest.map(str::to_string),
        latest_digest: Some(remote_digest),
        checked_at: now,
    })
}

pub async fn check_app(platform: &Platform, app_id: ShortId) -> anyhow::Result<ImageUpdateStatus> {
    let app = platform.app(app_id).await?;
    let AppSource::Image { image } = &app.spec.source else {
        anyhow::bail!("app {} does not have an image source", app.id);
    };

    let running_id = platform.store.running_deployment(app_id).await?;
    let running_digest = match running_id {
        Some(id) => platform
            .store
            .deployment(id)
            .await?
            .and_then(|d| d.image_digest),
        None => None,
    };

    let status = check_image_update(image, running_digest.as_deref()).await?;
    platform.store.set_image_update(app_id, &status).await?;
    Ok(status)
}

pub async fn auto_deploy_if_needed(
    platform: &Arc<Platform>,
    app: &AppEntry,
    status: &ImageUpdateStatus,
) -> anyhow::Result<()> {
    if !app.spec.auto_deploy || !status.update_available {
        return Ok(());
    }
    let AppSource::Image { image } = &app.spec.source else {
        return Ok(());
    };

    if let Some(latest_tag) = &status.latest_tag {
        let new_ref_str = format!("{}:{}", image.repository(), latest_tag);
        if let Ok(new_ref) = new_ref_str.parse::<ImageRef>() {
            let mut spec = app.spec.clone();
            spec.source = AppSource::Image { image: new_ref };
            let request = AppRequest {
                name: app.name.clone(),
                target: app.target,
                spec,
            };
            platform.update_app(app.id, request, unix_ms()).await?;
        }
    }

    info!(app = %app.name, "triggering auto-deployment for updated image");
    platform
        .deploy(app.id, "auto_update", DeploymentTrigger::Auto)
        .await?;
    Ok(())
}

async fn check_all_apps(platform: &Arc<Platform>) {
    let Ok(apps) = platform.store.apps().await else {
        return;
    };
    for app in apps {
        if !matches!(app.spec.source, AppSource::Image { .. }) {
            continue;
        }
        match check_app(platform, app.id).await {
            Ok(status) => {
                if let Err(err) = auto_deploy_if_needed(platform, &app, &status).await {
                    error!(app = %app.name, err = format!("{err:#}"), "auto-deploy failed");
                }
            }
            Err(err) => {
                warn!(app = %app.name, err = format!("{err:#}"), "image update check failed");
            }
        }
    }
}

pub fn spawn_checker(platform: &Arc<Platform>) {
    let platform = Arc::downgrade(platform);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let Some(platform) = platform.upgrade() else {
                return;
            };
            check_all_apps(&platform).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_image_target_handles_various_formats() {
        let nginx: ImageRef = "nginx".parse().unwrap();
        let (host, repo, tag) = parse_image_target(&nginx);
        assert_eq!(host, "registry-1.docker.io");
        assert_eq!(repo, "library/nginx");
        assert_eq!(tag, "latest");

        let custom: ImageRef = "ghcr.io/getzola/zola:v0.19.2".parse().unwrap();
        let (host, repo, tag) = parse_image_target(&custom);
        assert_eq!(host, "ghcr.io");
        assert_eq!(repo, "getzola/zola");
        assert_eq!(tag, "v0.19.2");

        let docker_io: ImageRef = "docker.io/library/alpine:3.18".parse().unwrap();
        let (host, repo, tag) = parse_image_target(&docker_io);
        assert_eq!(host, "registry-1.docker.io");
        assert_eq!(repo, "library/alpine");
        assert_eq!(tag, "3.18");
    }

    #[test]
    fn tag_parsing_and_version_comparison() {
        let tags = vec![
            "1.26.0".to_string(),
            "1.26.1".to_string(),
            "1.27.0".to_string(),
            "1.27.1".to_string(),
            "1.27.1-alpine".to_string(),
            "latest".to_string(),
        ];
        assert_eq!(find_newer_tag("1.26.0", &tags), Some("1.27.1".to_string()));
        assert_eq!(
            find_newer_tag("1.27.0-alpine", &tags),
            Some("1.27.1-alpine".to_string())
        );
        assert_eq!(find_newer_tag("1.27.1", &tags), None);
        assert_eq!(find_newer_tag("latest", &tags), None);

        let v_tags = vec![
            "v0.19.0".to_string(),
            "v0.19.2".to_string(),
            "v0.23.6".to_string(),
        ];
        assert_eq!(
            find_newer_tag("v0.19.0", &v_tags),
            Some("v0.23.6".to_string())
        );
    }

    #[test]
    fn auth_challenge_parsing() {
        let header = "Bearer realm=\"https://auth.docker.io/token\",service=\"registry.docker.io\",scope=\"repository:library/nginx:pull\"";
        let (realm, service, scope) = parse_auth_challenge(header).unwrap();
        assert_eq!(realm, "https://auth.docker.io/token");
        assert_eq!(service.as_deref(), Some("registry.docker.io"));
        assert_eq!(scope.as_deref(), Some("repository:library/nginx:pull"));
    }

    #[test]
    fn digest_hash_extraction() {
        assert_eq!(hash_of_digest("nginx@sha256:abcdef"), "sha256:abcdef");
        assert_eq!(hash_of_digest("sha256:abcdef"), "sha256:abcdef");
    }
}
