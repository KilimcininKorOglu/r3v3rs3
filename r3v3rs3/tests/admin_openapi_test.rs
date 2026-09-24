use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use reqwest::header::{CONTENT_TYPE, COOKIE};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener, with_server};

/// Every route of the admin API. The OpenAPI document must list exactly these operations.
const OPERATIONS: [(&str, &str); 81] = [
    ("post", "/api/login"),
    ("get", "/api/logout"),
    ("get", "/api/events"),
    ("get", "/api/session"),
    ("get", "/api/accounts"),
    ("post", "/api/accounts"),
    ("put", "/api/accounts/{username}"),
    ("delete", "/api/accounts/{username}"),
    ("get", "/api/audit"),
    ("get", "/api/config"),
    ("put", "/api/config"),
    ("post", "/api/config/notifications/test"),
    ("get", "/api/ports"),
    ("post", "/api/ports"),
    ("get", "/api/ports/{id}"),
    ("put", "/api/ports/{id}"),
    ("delete", "/api/ports/{id}"),
    ("get", "/api/ports/{id}/status"),
    ("get", "/api/ports/{id}/reset"),
    ("get", "/api/ports/interfaces"),
    ("get", "/api/proxies"),
    ("post", "/api/proxies"),
    ("get", "/api/proxies/{id}"),
    ("put", "/api/proxies/{id}"),
    ("delete", "/api/proxies/{id}"),
    ("get", "/api/proxies/{id}/status"),
    ("delete", "/api/proxies/{id}/cache"),
    ("get", "/api/access_lists"),
    ("post", "/api/access_lists"),
    ("put", "/api/access_lists/{id}"),
    ("delete", "/api/access_lists/{id}"),
    ("get", "/api/certs"),
    ("post", "/api/certs/self_sign"),
    ("post", "/api/certs/upload"),
    ("post", "/api/certs/delete"),
    ("get", "/api/certs/{id}"),
    ("delete", "/api/certs/{id}"),
    ("get", "/api/certs/{id}/download"),
    ("get", "/api/acme"),
    ("post", "/api/acme"),
    ("get", "/api/acme/{id}"),
    ("put", "/api/acme/{id}"),
    ("delete", "/api/acme/{id}"),
    ("get", "/api/logs/{id}"),
    ("get", "/api/app_info"),
    ("get", "/api/cdn"),
    ("post", "/api/cdn/refresh"),
    ("get", "/api/discovery"),
    ("get", "/api/cluster/status"),
    ("get", "/api/targets"),
    ("post", "/api/targets"),
    ("post", "/api/targets/{id}/token"),
    ("delete", "/api/targets/{id}"),
    ("get", "/api/apps"),
    ("post", "/api/apps"),
    ("get", "/api/apps/{id}"),
    ("put", "/api/apps/{id}"),
    ("delete", "/api/apps/{id}"),
    ("get", "/api/apps/{id}/env"),
    ("put", "/api/apps/{id}/env"),
    ("put", "/api/apps/{id}/git_token"),
    ("delete", "/api/apps/{id}/git_token"),
    ("post", "/api/apps/{id}/webhook_secret"),
    ("delete", "/api/apps/{id}/webhook_secret"),
    ("post", "/hooks/apps/{id}"),
    ("get", "/api/apps/{id}/deployments"),
    ("get", "/api/apps/{id}/logs"),
    ("post", "/api/apps/{id}/deploy"),
    ("get", "/api/deployments/{id}"),
    ("post", "/api/deployments/{id}/rollback"),
    ("get", "/api/git/connections"),
    ("post", "/api/git/connections"),
    ("get", "/api/git/connections/{id}"),
    ("put", "/api/git/connections/{id}"),
    ("delete", "/api/git/connections/{id}"),
    ("post", "/api/git/connections/{id}/authorize"),
    ("get", "/api/git/connections/{id}/repositories"),
    ("get", "/api/git/connections/{id}/branches"),
    ("get", "/oauth/git/callback"),
    ("get", "/api/platform/settings"),
    ("put", "/api/platform/settings"),
];

const HTTP_METHODS: [&str; 5] = ["get", "put", "post", "delete", "patch"];

#[tokio::test]
async fn openapi_document_and_swagger_ui_require_a_session() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-admin-openapi-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .accounts(HashMap::from([("admin".to_string(), "secret".to_string())]))
        .build();

    let app_info = new_appinfo(&dir, &dir);
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(addr).await?;
        let client = Client::new();

        assert_eq!(status(&client, addr, "/api/openapi.json", None).await?, 401);
        assert_eq!(status(&client, addr, "/api/docs/", None).await?, 401);

        let cookie = admin_session_cookie(addr).await?;
        let document: Value = client
            .get(format!("http://{addr}/api/openapi.json"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        check_document(&document);

        let docs = client
            .get(format!("http://{addr}/api/docs/"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(docs.status(), StatusCode::OK);
        assert!(content_type(&docs).starts_with("text/html"));

        // The routes still answer after the router records them in the document.
        assert_eq!(
            status(&client, addr, "/api/ports", Some(&cookie)).await?,
            200
        );
        let events = client
            .get(format!("http://{addr}/api/events"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(events.status(), StatusCode::OK);
        assert!(content_type(&events).starts_with("text/event-stream"));
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

fn check_document(document: &Value) {
    let expected = OPERATIONS
        .iter()
        .map(|(method, path)| (method.to_string(), path.to_string()))
        .collect::<BTreeSet<_>>();
    let operations = operations(document);
    let listed = operations
        .iter()
        .map(|(method, path, _)| (method.clone(), path.clone()))
        .collect::<BTreeSet<_>>();
    assert_eq!(listed, expected);

    let operation_ids = operations
        .iter()
        .map(|(_, _, id)| id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        operation_ids.len(),
        OPERATIONS.len(),
        "operation ids repeat"
    );

    assert_eq!(document["info"]["version"], env!("CARGO_PKG_VERSION"));
    let session = &document["components"]["securitySchemes"]["session"];
    assert_eq!(session["in"], "cookie");
    assert_eq!(session["name"], "token");

    let mut references = Vec::new();
    collect_references(document, &mut references);
    assert!(!references.is_empty());
    for reference in references {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .unwrap_or_else(|| panic!("unexpected reference {reference}"));
        assert!(
            document["components"]["schemas"].get(name).is_some(),
            "missing schema {name}"
        );
    }
}

/// Returns the method, the path and the operation id of every operation in the document.
fn operations(document: &Value) -> Vec<(String, String, String)> {
    let paths = document["paths"].as_object().expect("paths is an object");
    paths
        .iter()
        .flat_map(|(path, item)| {
            HTTP_METHODS.iter().filter_map(move |method| {
                let operation = item.get(*method)?;
                let id = operation["operationId"].as_str()?.to_string();
                Some((method.to_string(), path.clone(), id))
            })
        })
        .collect()
}

fn collect_references(value: &Value, references: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref") {
                references.push(reference.clone());
            }
            map.values()
                .for_each(|child| collect_references(child, references));
        }
        Value::Array(items) => items
            .iter()
            .for_each(|child| collect_references(child, references)),
        _ => {}
    }
}

async fn status(
    client: &Client,
    addr: SocketAddr,
    path: &str,
    cookie: Option<&str>,
) -> anyhow::Result<u16> {
    let mut request = client.get(format!("http://{addr}{path}"));
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    Ok(request.send().await?.status().as_u16())
}

fn content_type(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}
