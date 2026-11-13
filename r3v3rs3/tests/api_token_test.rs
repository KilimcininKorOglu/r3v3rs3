use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::auth::{ApiTokenCreated, ApiTokenInfo, CreateApiTokenRequest, TokenScope};
use reqwest::header::COOKIE;
use reqwest::{Client, StatusCode};
use std::{collections::HashMap, net::SocketAddr};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener, with_server};

#[tokio::test]
async fn api_token_scoped_access_and_lifecycle() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-api-token-test-{}", std::process::id()));
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
        manage_the_tokens(addr).await
    })
    .await?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Runs every token check in order: sign-in, creation, list, scopes and deletion.
async fn manage_the_tokens(addr: SocketAddr) -> anyhow::Result<()> {
    let client = Client::new();

    // 1. Unauthenticated request to /api/proxies is rejected
    let resp = client
        .get(format!("http://{addr}/api/proxies"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 2. Sign in as admin
    let cookie = admin_session_cookie(addr).await?;

    // 3. Create a Read-scoped API token
    let read_token = create_token(
        &client,
        addr,
        &cookie,
        "monitoring-script",
        TokenScope::Read,
    )
    .await?;
    assert_eq!(read_token.name, "monitoring-script");
    assert_eq!(read_token.scope, TokenScope::Read);
    assert!(read_token.token.starts_with("r3v_"));

    // 4. Create an Edit-scoped API token
    let edit_token =
        create_token(&client, addr, &cookie, "deploy-pipeline", TokenScope::Edit).await?;
    assert_eq!(read_token.scope, TokenScope::Read);
    assert_eq!(edit_token.scope, TokenScope::Edit);

    check_token_list(&client, addr, &cookie, &read_token, &edit_token).await?;
    check_token_scopes(&client, addr, &read_token, &edit_token).await?;
    check_deleted_token(&client, addr, &cookie, &read_token).await
}

/// Creates an API token with the session cookie and checks that the request succeeds.
async fn create_token(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    name: &str,
    scope: TokenScope,
) -> anyhow::Result<ApiTokenCreated> {
    let request = CreateApiTokenRequest {
        name: name.into(),
        scope,
    };
    let resp = client
        .post(format!("http://{addr}/api/auth/tokens"))
        .header(COOKIE, cookie)
        .json(&request)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    Ok(resp.json().await?)
}

/// Checks that the token list holds both tokens with their names.
async fn check_token_list(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    read_token: &ApiTokenCreated,
    edit_token: &ApiTokenCreated,
) -> anyhow::Result<()> {
    // 5. List tokens
    let resp = client
        .get(format!("http://{addr}/api/auth/tokens"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let tokens: Vec<ApiTokenInfo> = resp.json().await?;
    assert_eq!(tokens.len(), 2);
    assert!(
        tokens
            .iter()
            .any(|t| t.id == read_token.id && t.name == "monitoring-script")
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.id == edit_token.id && t.name == "deploy-pipeline")
    );
    Ok(())
}

/// Sends a GET request with the bearer token and returns the status.
async fn get_with_token(
    client: &Client,
    addr: SocketAddr,
    path: &str,
    token: &str,
) -> anyhow::Result<StatusCode> {
    Ok(client
        .get(format!("http://{addr}{path}"))
        .bearer_auth(token)
        .send()
        .await?
        .status())
}

/// Checks that the Read token only reads, and that the Edit token changes the proxies and reads
/// the metrics.
async fn check_token_scopes(
    client: &Client,
    addr: SocketAddr,
    read_token: &ApiTokenCreated,
    edit_token: &ApiTokenCreated,
) -> anyhow::Result<()> {
    // 6. Test Read-scoped token: GET /api/proxies works
    let status = get_with_token(client, addr, "/api/proxies", &read_token.token).await?;
    assert_eq!(status, StatusCode::OK);

    // 7. Test Read-scoped token: POST /api/proxies is forbidden (403)
    let proxy_entry =
        common::http_proxy_entry("p1", "port1", r3v3rs3_api::proxy::HttpProxy::default());
    let resp = client
        .post(format!("http://{addr}/api/proxies"))
        .bearer_auth(&read_token.token)
        .json(&proxy_entry)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 8. Test Edit-scoped token: POST /api/proxies passes permission check
    let resp = client
        .post(format!("http://{addr}/api/proxies"))
        .bearer_auth(&edit_token.token)
        .json(&proxy_entry)
        .send()
        .await?;
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);

    // 9. Test /api/metrics with Edit token
    let status = get_with_token(client, addr, "/api/metrics", &edit_token.token).await?;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}

/// Deletes the Read token and checks that it no longer works.
async fn check_deleted_token(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    read_token: &ApiTokenCreated,
) -> anyhow::Result<()> {
    // 10. Delete the Read token
    let resp = client
        .delete(format!("http://{addr}/api/auth/tokens/{}", read_token.id))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    // 11. Read token no longer works (401 Unauthorized)
    let status = get_with_token(client, addr, "/api/proxies", &read_token.token).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}
