//! The ACME DNS-01 flow against Pebble, from the order to the stored certificate.
//!
//! The test needs the containers of `tests/pebble/docker-compose.yml`, and `SSL_CERT_FILE`
//! must name the Pebble test CA. `make test-acme-pebble` does both.

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use r3v3rs3::{
    certs::Cert,
    command::ServerCommand,
    config::storage::Storage,
    discovery::{DiscoveredProxy, DiscoverySnapshot, ProxyDefinition},
    server::{
        rpc::acme::{AddAcme, GetAcmeList},
        ServerChannels,
    },
};
use r3v3rs3_api::{
    acme::{Acme, AcmeConfig, AcmeRequest, DnsProvider, DNS_01},
    app::AppConfig,
    discovery::{DiscoveryProvider, DiscoverySource, DiscoveryState},
    id::ShortId,
    port::PortEntry,
    proxy::{HttpProxy, ProxyKind},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

mod common;
use common::{
    alloc_tcp_port, call, http_port_entry, http_route, serve_http_upstream, wait_for_listener,
    with_server, TestStorage,
};

const PEBBLE_DIRECTORY: &str = "https://localhost:8470/dir";
const CHALLTESTSRV_DNS: &str = "127.0.0.1:8471";
const CHALLTESTSRV_MANAGEMENT: &str = "http://127.0.0.1:8472";
const CERT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Provider {
    /// TXT records by record ID, as `(name, value)`.
    records: Mutex<HashMap<String, (String, String)>>,
    deleted: Mutex<Vec<String>>,
}

async fn challtestsrv(action: &str, body: Value) -> Result<(), String> {
    reqwest::Client::new()
        .post(format!("{CHALLTESTSRV_MANAGEMENT}/{action}"))
        .json(&body)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map(|_| ())
        .map_err(|err| err.to_string())
}

async fn create_record(
    State(provider): State<Arc<Provider>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, String> {
    let name = body["name"].as_str().unwrap_or_default().to_string();
    let value = body["content"].as_str().unwrap_or_default().to_string();
    challtestsrv(
        "set-txt",
        json!({ "host": format!("{name}."), "value": value }),
    )
    .await?;
    let mut records = provider.records.lock().unwrap();
    let id = format!("rec-{}", records.len() + 1);
    records.insert(id.clone(), (name, value));
    Ok(Json(json!({ "result": { "id": id } })))
}

async fn delete_record(
    State(provider): State<Arc<Provider>>,
    Path((_, id)): Path<(String, String)>,
) -> Result<Json<Value>, String> {
    let record = provider.records.lock().unwrap().get(&id).cloned();
    let Some((name, _)) = record else {
        return Err(format!("unknown record {id}"));
    };
    challtestsrv("clear-txt", json!({ "host": format!("{name}.") })).await?;
    provider.deleted.lock().unwrap().push(id.clone());
    Ok(Json(json!({ "result": { "id": id } })))
}

/// A Cloudflare API that publishes the TXT records on pebble-challtestsrv.
async fn start_provider() -> anyhow::Result<(String, Arc<Provider>)> {
    let provider = Arc::new(Provider::default());
    let app = Router::new()
        .route(
            "/zones",
            get(|| async { Json(json!({ "result": [{ "id": "zone-1" }] })) }),
        )
        .route("/zones/{zone}/dns_records", post(create_record))
        .route("/zones/{zone}/dns_records/{id}", delete(delete_record))
        .with_state(provider.clone());
    let url = serve_http_upstream(app).await?;
    Ok((url.as_str().trim_end_matches('/').to_string(), provider))
}

/// Waits for a stored certificate whose sorted subject names are `san`.
async fn wait_for_cert(storage: &TestStorage, san: &[&str]) -> anyhow::Result<Arc<Cert>> {
    let deadline = tokio::time::Instant::now() + CERT_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let found = storage.load_certs().await.into_iter().find(|cert| {
            let mut names = cert.san.iter().map(ToString::to_string).collect::<Vec<_>>();
            names.sort();
            names == san
        });
        if let Some(cert) = found {
            return Ok(cert);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!(
        "no certificate for {san:?} after {} seconds",
        CERT_TIMEOUT.as_secs()
    )
}

/// Starts the Cloudflare API and returns it with a storage and a DNS-01 request for Pebble.
async fn pebble_setup(
    identifiers: &[&str],
    ports: Vec<PortEntry>,
) -> anyhow::Result<(Arc<Provider>, TestStorage, AcmeRequest)> {
    // The server log is the only place an order failure shows up.
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    wait_for_listener("127.0.0.1:8470".parse()?).await?;
    let (provider_url, provider) = start_provider().await?;

    let config = AppConfig {
        dns_challenge_resolver: Some(CHALLTESTSRV_DNS.parse()?),
        ..Default::default()
    };
    let storage = TestStorage::builder().config(config).ports(ports).build();
    let request = AcmeRequest {
        server_url: PEBBLE_DIRECTORY.to_string(),
        contacts: vec![],
        eab: None,
        acme: Acme {
            config: AcmeConfig {
                provider: "Pebble".to_string(),
                ..Default::default()
            },
            identifiers: identifiers
                .iter()
                .map(|name| name.parse())
                .collect::<Result<_, _>>()?,
            challenge_type: DNS_01.to_string(),
            dns_provider: Some(DnsProvider::Cloudflare {
                api_token: "test-token".to_string(),
                api_url: Some(provider_url),
            }),
        },
    };
    Ok((provider, storage, request))
}

#[tokio::test]
#[ignore = "needs the Pebble containers, run it with make test-acme-pebble"]
async fn dns_01_issues_a_wildcard_certificate() -> anyhow::Result<()> {
    let (provider, storage, request) =
        pebble_setup(&["example.test", "*.example.test"], vec![]).await?;
    let cert_storage = storage.clone();
    with_server(storage, move |mut channels| async move {
        if let Err(err) = call(&mut channels, AddAcme { request }).await? {
            anyhow::bail!("AddAcme failed: {err}");
        }
        wait_for_cert(&cert_storage, &["*.example.test", "example.test"]).await?;
        Ok(())
    })
    .await?;

    let mut deleted = provider.deleted.lock().unwrap().clone();
    deleted.sort();
    assert_eq!(deleted, vec!["rec-1", "rec-2"]);
    Ok(())
}

/// Sends a Docker snapshot with one proxy for `app.site.test` that names the ACME entry.
async fn send_running_snapshot(
    channels: &mut ServerChannels,
    acme_id: ShortId,
) -> anyhow::Result<()> {
    let http = HttpProxy {
        vhosts: vec!["app.site.test".parse()?],
        routes: vec![http_route("/", "http://127.0.0.1:1/", None)],
        ..Default::default()
    };
    let proxy = DiscoveredProxy {
        key: "app/http.app".into(),
        source: DiscoverySource {
            provider: DiscoveryProvider::Docker,
            resource: "app".into(),
        },
        definition: ProxyDefinition {
            key: "http.app".into(),
            name: "app".into(),
            ports: vec!["web".into()],
            active: true,
            acme: Some(acme_id),
            kind: ProxyKind::Http(Box::new(http)),
        },
    };
    let snapshot = DiscoverySnapshot {
        provider: DiscoveryProvider::Docker,
        generation: 0,
        state: DiscoveryState::Running,
        error: None,
        proxies: Some(vec![proxy]),
        certs: vec![],
        issues: vec![],
    };
    channels
        .command
        .send(ServerCommand::SetDiscovery { snapshot })
        .await?;
    Ok(())
}

/// The names differ from the other test, so the TXT records of the parallel tests do not collide.
#[tokio::test]
#[ignore = "needs the Pebble containers, run it with make test-acme-pebble"]
async fn a_discovered_host_gets_its_own_certificate_from_the_entry() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let mut port = http_port_entry("web", &proxy_port);
    port.port.name = "web".into();
    let (_provider, storage, request) = pebble_setup(&["site.test"], vec![port]).await?;
    let cert_storage = storage.clone();
    with_server(storage, move |mut channels| async move {
        if let Err(err) = call(&mut channels, AddAcme { request }).await? {
            anyhow::bail!("AddAcme failed: {err}");
        }
        let entries = call(&mut channels, GetAcmeList).await??;
        let acme_id = entries
            .first()
            .ok_or_else(|| anyhow::anyhow!("no ACME entry"))?
            .id;
        let entry_cert = wait_for_cert(&cert_storage, &["site.test"]).await?;

        send_running_snapshot(&mut channels, acme_id).await?;
        let host_cert = wait_for_cert(&cert_storage, &["app.site.test"]).await?;
        let acme_ids =
            [&entry_cert, &host_cert].map(|cert| cert.metadata.as_ref().map(|m| m.acme_id));
        assert_eq!(acme_ids, [Some(acme_id), Some(acme_id)]);

        // Each target has its own certificate, so the same snapshot starts no other order.
        send_running_snapshot(&mut channels, acme_id).await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(cert_storage.load_certs().await.len(), 2);
        Ok(())
    })
    .await
}
