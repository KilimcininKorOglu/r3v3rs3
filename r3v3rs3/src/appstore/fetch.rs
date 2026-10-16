//! Downloads the app store catalog from the repository.

use super::set_last_error;
use crate::cdn::fetch::{REQUEST_TIMEOUT, build_client};
use crate::command::ServerCommand;
use anyhow::bail;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, header::USER_AGENT};
use r3v3rs3_api::store::StoreCatalog;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

/// How often the catalog is downloaded.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
const STARTUP_DELAY: Duration = Duration::from_secs(10);
/// The catalog of the repository, refreshed on every change of `main`.
const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/data/app-store.json";
/// The catalog stays far below the CDN list limit, but shares its bound.
const MAX_BODY_SIZE: usize = 4 * 1024 * 1024;

/// Downloads the catalog from the repository.
pub async fn fetch_catalog() -> anyhow::Result<StoreCatalog> {
    let client = build_client().await?;
    let request = Request::builder()
        .uri(CATALOG_URL)
        .header(USER_AGENT, concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")))
        .body(Full::new(Bytes::new()))?;
    let response = tokio::time::timeout(REQUEST_TIMEOUT, client.request(request)).await??;
    if !response.status().is_success() {
        bail!("unexpected status {}", response.status());
    }
    let body = tokio::time::timeout(
        REQUEST_TIMEOUT,
        Limited::new(response.into_body(), MAX_BODY_SIZE).collect(),
    )
    .await?
    .map_err(|err| anyhow::anyhow!(err))?
    .to_bytes();
    parse_catalog(std::str::from_utf8(&body)?)
}

fn parse_catalog(text: &str) -> anyhow::Result<StoreCatalog> {
    let catalog: StoreCatalog = serde_json::from_str(text)?;
    if catalog.apps.is_empty() {
        bail!("the catalog holds no apps");
    }
    Ok(catalog)
}

/// Refreshes the catalog every day and sends it to the server.
pub fn spawn_refresh_task(command: mpsc::Sender<ServerCommand>) {
    tokio::spawn(async move {
        let mut wait = initial_wait(crate::appstore::state().catalog.updated_at);
        loop {
            tokio::time::sleep(wait).await;
            wait = match fetch_catalog().await {
                Ok(catalog) => {
                    info!(apps = catalog.apps.len(), "refreshed the app store catalog");
                    if command
                        .send(ServerCommand::SetAppStore { catalog })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    REFRESH_INTERVAL
                }
                Err(err) => {
                    error!(%err, "failed to refresh the app store catalog");
                    set_last_error(err.to_string());
                    RETRY_INTERVAL
                }
            };
        }
    });
}

/// Waits until the next refresh is due, but never shorter than the startup delay.
fn initial_wait(updated_at: i64) -> Duration {
    let due = updated_at.saturating_add(REFRESH_INTERVAL.as_secs() as i64);
    let now = i64::try_from(crate::clock::unix_ms() / 1000).unwrap_or(i64::MAX);
    let remaining = u64::try_from(due.saturating_sub(now)).unwrap_or(0);
    Duration::from_secs(remaining).max(STARTUP_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_catalog() {
        let text = r#"{"version":1,"updated_at":10,"apps":[
            {"id":"whoami","name":"Whoami","description":"test","image":"traefik/whoami:v1.10.3","port":80}
        ]}"#;
        let catalog = parse_catalog(text).unwrap();
        assert_eq!(catalog.apps.len(), 1);
        assert_eq!(catalog.apps[0].id, "whoami");
    }

    #[test]
    fn rejects_an_empty_catalog() {
        let text = r#"{"version":1,"updated_at":10,"apps":[]}"#;
        assert!(parse_catalog(text).is_err());
    }

    #[test]
    fn rejects_a_broken_catalog() {
        assert!(parse_catalog("not json").is_err());
        let text = r#"{"version":1,"updated_at":10,"apps":[{"id":"a"}]}"#;
        assert!(parse_catalog(text).is_err());
    }

    #[test]
    fn initial_wait_is_never_shorter_than_startup_delay() {
        assert_eq!(initial_wait(0), STARTUP_DELAY);
        let now = i64::try_from(crate::clock::unix_ms() / 1000).unwrap_or(i64::MAX);
        assert!(initial_wait(now) > REFRESH_INTERVAL - Duration::from_secs(60));
    }
}
