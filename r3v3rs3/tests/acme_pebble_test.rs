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
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{acme::AddAcme, ErasedRpcMethod, RpcWrapper},
};
use r3v3rs3_api::{
    acme::{Acme, AcmeConfig, AcmeRequest, DnsProvider, DNS_01},
    app::AppConfig,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

mod common;
use common::{serve_http_upstream, wait_for_listener, with_server, TestStorage};

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

async fn wait_for_cert(storage: &TestStorage) -> anyhow::Result<Vec<String>> {
    let deadline = tokio::time::Instant::now() + CERT_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Some(cert) = storage.load_certs().await.first() {
            return Ok(cert.san.iter().map(ToString::to_string).collect());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!("no certificate after {} seconds", CERT_TIMEOUT.as_secs())
}

#[tokio::test]
#[ignore = "needs the Pebble containers, run it with make test-acme-pebble"]
async fn dns_01_issues_a_wildcard_certificate() -> anyhow::Result<()> {
    // The server log is the only place an order failure shows up.
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    wait_for_listener("127.0.0.1:8470".parse()?).await?;
    let (provider_url, provider) = start_provider().await?;

    let config = AppConfig {
        dns_challenge_resolver: Some(CHALLTESTSRV_DNS.parse()?),
        ..Default::default()
    };
    let storage = TestStorage::builder().config(config).build();
    let request = AcmeRequest {
        server_url: PEBBLE_DIRECTORY.to_string(),
        contacts: vec![],
        eab: None,
        acme: Acme {
            config: AcmeConfig {
                provider: "Pebble".to_string(),
                ..Default::default()
            },
            identifiers: vec!["example.test".parse()?, "*.example.test".parse()?],
            challenge_type: DNS_01.to_string(),
            dns_provider: Some(DnsProvider::Cloudflare {
                api_token: "test-token".to_string(),
                api_url: Some(provider_url),
            }),
        },
    };

    let cert_storage = storage.clone();
    with_server(storage, move |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(AddAcme { request })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod { id: 1, arg })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        if let Err(err) = callback.result {
            anyhow::bail!("AddAcme failed: {err}");
        }

        let mut san = wait_for_cert(&cert_storage).await?;
        san.sort();
        assert_eq!(san, vec!["*.example.test", "example.test"]);
        Ok(())
    })
    .await?;

    let mut deleted = provider.deleted.lock().unwrap().clone();
    deleted.sort();
    assert_eq!(deleted, vec!["rec-1", "rec-2"]);
    Ok(())
}
