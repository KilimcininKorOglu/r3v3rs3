//! Downloads the published edge IP ranges of known CDNs.

use super::{set_last_errors, table, CdnRanges};
use crate::{command::ServerCommand, proxy::http::hyper_tls::client::HttpsConnector};
use anyhow::{anyhow, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{header::CONTENT_TYPE, header::USER_AGENT, Method, Request};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use ipnet::IpNet;
use r3v3rs3_api::{cdn::CdnProvider, cidr::parse_cidr};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing::{error, info, warn};

/// How often the ranges are downloaded.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
const STARTUP_DELAY: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_SIZE: usize = 4 * 1024 * 1024;

/// Google Cloud Load Balancing proxy and health check ranges. Google publishes them only in its
/// documentation, so they are not downloaded.
const GOOGLE_CLOUD_RANGES: &[&str] = &[
    "130.211.0.0/22",
    "35.191.0.0/16",
    "2600:2d00:1:b029::/64",
    "2600:2d00:1:1::/64",
];

#[derive(Debug, Clone, Copy)]
enum Format {
    /// One CIDR block or address per line.
    Lines,
    /// JSON arrays of strings under the given keys. No keys means a top-level array.
    Json(&'static [&'static str]),
}

struct Source {
    provider: CdnProvider,
    url: &'static str,
    form: Option<&'static str>,
    format: Format,
}

const SOURCES: &[Source] = &[
    Source {
        provider: CdnProvider::Cloudflare,
        url: "https://www.cloudflare.com/ips-v4",
        form: None,
        format: Format::Lines,
    },
    Source {
        provider: CdnProvider::Cloudflare,
        url: "https://www.cloudflare.com/ips-v6",
        form: None,
        format: Format::Lines,
    },
    Source {
        provider: CdnProvider::Fastly,
        url: "https://api.fastly.com/public-ip-list",
        form: None,
        format: Format::Json(&["addresses", "ipv6_addresses"]),
    },
    Source {
        provider: CdnProvider::Cloudfront,
        url: "https://d7uri8nf7uskq.cloudfront.net/tools/list-cloudfront-ips",
        form: None,
        format: Format::Json(&[
            "CLOUDFRONT_GLOBAL_IP_LIST",
            "CLOUDFRONT_REGIONAL_EDGE_IP_LIST",
        ]),
    },
    Source {
        provider: CdnProvider::Bunny,
        url: "https://api.bunny.net/system/edgeserverlist/plain",
        form: None,
        format: Format::Lines,
    },
    Source {
        provider: CdnProvider::Bunny,
        url: "https://api.bunny.net/system/edgeserverlist/ipv6",
        form: None,
        format: Format::Json(&[]),
    },
    Source {
        provider: CdnProvider::Gcore,
        url: "https://api.gcore.com/cdn/public-ip-list",
        form: None,
        format: Format::Json(&["addresses", "addresses_v6"]),
    },
    Source {
        provider: CdnProvider::Keycdn,
        url: "https://www.keycdn.com/shield-prefixes.json",
        form: None,
        format: Format::Json(&["prefixes"]),
    },
    Source {
        provider: CdnProvider::Imperva,
        url: "https://my.imperva.com/api/integration/v1/ips",
        form: Some("resp_format=text"),
        format: Format::Lines,
    },
];

pub(crate) type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

pub struct FetchResult {
    pub ranges: CdnRanges,
    /// Errors of the providers that kept their previous ranges.
    pub errors: Vec<String>,
}

/// Downloads all providers. A provider that fails keeps its ranges from `previous`.
pub async fn fetch_all(previous: &CdnRanges) -> anyhow::Result<FetchResult> {
    let client = build_client().await?;
    let mut providers: BTreeMap<CdnProvider, Vec<IpNet>> = BTreeMap::new();
    let mut failed = BTreeSet::new();
    let mut errors = Vec::new();

    for source in SOURCES {
        match fetch_source(&client, source).await {
            Ok(nets) => providers.entry(source.provider).or_default().extend(nets),
            Err(err) => {
                failed.insert(source.provider);
                errors.push(format!("{}: {err}", source.provider));
            }
        }
    }

    let downloaded: BTreeSet<_> = SOURCES.iter().map(|source| source.provider).collect();
    if failed.len() == downloaded.len() {
        bail!("all providers failed: {}", errors.join("; "));
    }

    for provider in failed {
        match previous.providers.get(&provider) {
            Some(nets) => providers.insert(provider, nets.clone()),
            None => providers.remove(&provider),
        };
    }
    providers.insert(CdnProvider::GoogleCloud, google_cloud_ranges()?);
    for nets in providers.values_mut() {
        nets.sort();
        nets.dedup();
    }

    Ok(FetchResult {
        ranges: CdnRanges {
            updated_at: unix_now(),
            providers,
        },
        errors,
    })
}

/// Refreshes the ranges every day and sends them to the server.
pub fn spawn_refresh_task(command: mpsc::Sender<ServerCommand>) {
    tokio::spawn(async move {
        let mut wait = initial_wait(table().ranges().updated_at);
        loop {
            tokio::time::sleep(wait).await;
            let previous = table().ranges().clone();
            wait = match fetch_all(&previous).await {
                Ok(result) => {
                    info!(errors = result.errors.len(), "refreshed CDN IP ranges");
                    set_last_errors(result.errors);
                    let ranges = result.ranges;
                    if command
                        .send(ServerCommand::SetCdnRanges { ranges })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    REFRESH_INTERVAL
                }
                Err(err) => {
                    error!(%err, "failed to refresh CDN IP ranges");
                    set_last_errors(vec![err.to_string()]);
                    RETRY_INTERVAL
                }
            };
        }
    });
}

fn initial_wait(updated_at: i64) -> Duration {
    let due = updated_at.saturating_add(REFRESH_INTERVAL.as_secs() as i64);
    let remaining = u64::try_from(due.saturating_sub(unix_now())).unwrap_or(0);
    Duration::from_secs(remaining).max(STARTUP_DELAY)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn google_cloud_ranges() -> anyhow::Result<Vec<IpNet>> {
    GOOGLE_CLOUD_RANGES
        .iter()
        .map(|value| parse_cidr(value).map_err(|err| anyhow!(err)))
        .collect()
}

/// Builds an HTTPS client that trusts the native root certificates.
pub(crate) async fn build_client() -> anyhow::Result<HttpClient> {
    let native = tokio::task::spawn_blocking(rustls_native_certs::load_native_certs).await?;
    for err in native.errors {
        warn!(%err, "failed to load a native root certificate");
    }
    let mut roots = RootCertStore::empty();
    for cert in native.certs {
        if let Err(err) = roots.add(cert) {
            warn!(%err, "failed to add a native root certificate");
        }
    }
    if roots.is_empty() {
        bail!("no root certificates are available");
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Client::builder(TokioExecutor::new()).build(HttpsConnector::new(Arc::new(config))))
}

fn build_request(source: &Source) -> anyhow::Result<Request<Full<Bytes>>> {
    let builder = Request::builder()
        .uri(source.url)
        .header(USER_AGENT, concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")));
    let request = match source.form {
        Some(form) => builder
            .method(Method::POST)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Full::new(Bytes::from_static(form.as_bytes())))?,
        None => builder.method(Method::GET).body(Full::new(Bytes::new()))?,
    };
    Ok(request)
}

async fn fetch_source(client: &HttpClient, source: &Source) -> anyhow::Result<Vec<IpNet>> {
    let response =
        tokio::time::timeout(REQUEST_TIMEOUT, client.request(build_request(source)?)).await??;
    if !response.status().is_success() {
        bail!("unexpected status {}", response.status());
    }
    let body = tokio::time::timeout(
        REQUEST_TIMEOUT,
        Limited::new(response.into_body(), MAX_BODY_SIZE).collect(),
    )
    .await?
    .map_err(|err| anyhow!(err))?
    .to_bytes();
    let nets = parse_list(std::str::from_utf8(&body)?, source.format)?;
    if nets.is_empty() {
        bail!("the list is empty");
    }
    Ok(nets)
}

fn parse_list(text: &str, format: Format) -> anyhow::Result<Vec<IpNet>> {
    let entries = match format {
        Format::Lines => text.split_whitespace().map(str::to_string).collect(),
        Format::Json(keys) => json_entries(text, keys)?,
    };
    let total = entries.len();
    let nets: Vec<IpNet> = entries
        .iter()
        .filter_map(|entry| parse_cidr(entry).ok())
        .collect();
    if nets.len() < total {
        warn!(
            skipped = total - nets.len(),
            "skipped invalid CDN IP range entries"
        );
    }
    Ok(nets)
}

fn json_entries(text: &str, keys: &[&str]) -> anyhow::Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    let arrays = if keys.is_empty() {
        vec![&value]
    } else {
        keys.iter()
            .map(|key| value.get(key).ok_or_else(|| anyhow!("missing key {key}")))
            .collect::<anyhow::Result<Vec<_>>>()?
    };
    let mut entries = Vec::new();
    for array in arrays {
        let items = array
            .as_array()
            .ok_or_else(|| anyhow!("expected a JSON array"))?;
        entries.extend(
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string)),
        );
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_lines() {
        let nets = parse_list("173.245.48.0/20\n89.187.188.227\n\n", Format::Lines).unwrap();
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[1].to_string(), "89.187.188.227/32");
    }

    #[test]
    fn parses_json_keys() {
        let text = r#"{"addresses":["23.235.32.0/20"],"ipv6_addresses":["2a04:4e40::/32"]}"#;
        let nets = parse_list(text, Format::Json(&["addresses", "ipv6_addresses"])).unwrap();
        assert_eq!(nets.len(), 2);
    }

    #[test]
    fn parses_top_level_json_array() {
        let nets = parse_list(r#"["2400:52e0:1500::714:1"]"#, Format::Json(&[])).unwrap();
        assert_eq!(nets[0].to_string(), "2400:52e0:1500::714:1/128");
    }

    #[test]
    fn rejects_missing_json_key() {
        assert!(parse_list(r#"{"prefixes":[]}"#, Format::Json(&["addresses"])).is_err());
    }

    #[test]
    fn skips_invalid_entries() {
        let nets = parse_list("10.0.0.0/8\nnot-an-ip\n", Format::Lines).unwrap();
        assert_eq!(nets.len(), 1);
    }

    #[test]
    fn initial_wait_is_never_shorter_than_startup_delay() {
        assert_eq!(initial_wait(0), STARTUP_DELAY);
        let recent = unix_now();
        assert!(initial_wait(recent) > REFRESH_INTERVAL - Duration::from_secs(60));
    }

    /// Run with `make cdn-snapshot` to update the snapshot compiled into the binary.
    #[tokio::test]
    #[ignore = "downloads the CDN IP ranges and rewrites data/cdn-ranges.json"]
    async fn update_embedded_snapshot() {
        let result = fetch_all(&CdnRanges::default()).await.unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let json = serde_json::to_string_pretty(&result.ranges).unwrap();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/cdn-ranges.json");
        std::fs::write(path, json + "\n").unwrap();
    }
}
