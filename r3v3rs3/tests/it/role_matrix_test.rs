//! Sends every route of the admin API that changes state as the lowest-permission account and
//! checks that the route refuses it. The expected statuses come from `docs/content/accounts.md`:
//! a viewer changes nothing but its own session, passkeys and tokens, and "an action that the
//! role does not allow gets `403 forbidden`".

use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::auth::Role;
use reqwest::{Client, Method, header::COOKIE};
use std::net::SocketAddr;
use tracing_subscriber::filter::LevelFilter;

use crate::admin_openapi_test::OPERATIONS;
use crate::common;
use common::{TestStorage, alloc_tcp_port, session_cookie, wait_for_listener, with_server};

/// The routes that a viewer may call although they are not GET routes, each with the reason.
const OWN_ACCOUNT_OR_PUBLIC: [&str; 15] = [
    // Signing in and out needs no role.
    "/api/login",
    "/api/logout",
    // Every account manages its own passkeys and API tokens.
    "/api/auth/passkey/register",
    "/api/auth/passkey/{id}",
    "/api/auth/tokens",
    "/api/auth/tokens/{id}",
    // These POST routes only compute a result from the request and the stored configuration,
    // which a viewer reads anyway.
    "/api/proxies/dry-run",
    "/api/proxies/{id}/grpc/reflect",
    "/api/proxies/{id}/grpc/health",
    "/api/certs/coverage",
    "/api/security/headers/analyze",
    // The OAuth endpoints of the MCP clients authorize with their own grants.
    "/oauth/authorize",
    "/oauth/token",
    "/oauth/register",
    // The push webhook of a Git provider authorizes with the signature of the app secret.
    "/hooks/apps/{id}",
];

/// Replaces each `{parameter}` of a route with a value that parses as an id.
fn concrete_path(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .map_or(rest.len(), |end| start + end + 1);
        out.push_str("abc");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// The routes that change state and that a viewer may not call.
fn forbidden_routes() -> Vec<(Method, &'static str)> {
    OPERATIONS
        .iter()
        .filter(|(method, _)| *method != "get")
        .filter(|(_, path)| !OWN_ACCOUNT_OR_PUBLIC.contains(path))
        .map(|(method, path)| (method.to_uppercase().parse().expect("method"), *path))
        .collect()
}

/// Sends each route as the account and returns the routes that did not answer 403.
async fn routes_not_forbidden(addr: SocketAddr, cookie: &str) -> anyhow::Result<Vec<String>> {
    let client = Client::new();
    let mut wrong = Vec::new();
    for (method, path) in forbidden_routes() {
        let status = client
            .request(
                method.clone(),
                format!("http://{addr}{}", concrete_path(path)),
            )
            .header(COOKIE, cookie)
            .json(&serde_json::json!({}))
            .send()
            .await?
            .status();
        if status.as_u16() != 403 {
            wrong.push(format!("{method} {path} -> {status}"));
        }
    }
    Ok(wrong)
}

/// Checks that the viewer still creates and deletes its own API token.
async fn check_own_token(addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    let client = Client::new();
    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/auth/tokens"))
        .header(COOKIE, cookie)
        .json(&serde_json::json!({"name": "viewer token"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = created["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no token id in {created}"))?;
    client
        .delete(format!("http://{addr}/api/auth/tokens/{id}"))
        .header(COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

#[tokio::test]
async fn a_viewer_cannot_call_any_route_that_changes_state() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-role-matrix-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .account("viewer", "viewer-secret", Role::Viewer, None)
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
        let cookie = session_cookie(addr, "viewer", "viewer-secret").await?;
        let wrong = routes_not_forbidden(addr, &cookie).await?;
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        check_own_token(addr, &cookie).await
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn concrete_path_fills_every_parameter() {
    assert_eq!(
        concrete_path("/api/projects/{id}/environments/{env_id}"),
        "/api/projects/abc/environments/abc"
    );
    assert_eq!(concrete_path("/api/ports"), "/api/ports");
}
