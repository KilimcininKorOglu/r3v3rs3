use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::auth::Role;
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    TestStorage, alloc_tcp_port, login_when_allowed, send, session_cookie, wait_for_listener,
    with_server,
};

const PATH: &str = "/api/access_lists";

async fn lists(addr: SocketAddr, cookie: &str) -> anyhow::Result<Vec<Value>> {
    let (status, body) = send(addr, Method::GET, PATH, cookie, None).await?;
    assert_eq!(status, 200, "{body}");
    Ok(serde_json::from_str(&body)?)
}

fn office(password: Option<&str>) -> Value {
    let mut user = json!({"username": "alice"});
    if let Some(password) = password {
        user["password"] = json!(password);
    }
    json!({
        "name": "Office",
        "ip_filter": {"allow": ["10.0.0.0/8"]},
        "auth": {"type": "basic", "users": [user]},
    })
}

#[tokio::test]
async fn an_editor_manages_the_access_lists_and_the_api_hides_their_secrets() -> anyhow::Result<()>
{
    let dir = std::env::temp_dir().join(format!("r3v3rs3-access-lists-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .account("editor", "editor-secret", Role::Editor, None)
        .account(
            "scoped",
            "scoped-secret",
            Role::Editor,
            Some(BTreeSet::new()),
        )
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
        let editor = session_cookie(addr, "editor", "editor-secret").await?;
        let scoped = session_cookie(addr, "scoped", "scoped-secret").await?;
        // The third sign-in waits for the login rate limit.
        let viewer = login_when_allowed(addr, "viewer", "viewer-secret").await?;

        // Only an account that changes the ports and the certificates changes the access lists.
        for cookie in [&scoped, &viewer] {
            let body = Some(office(Some("alice-secret")));
            let (status, _) = send(addr, Method::POST, PATH, cookie, body).await?;
            assert_eq!(status, 403);
        }
        let unnamed = Some(json!({"name": " "}));
        let (status, body) = send(addr, Method::POST, PATH, &editor, unnamed).await?;
        assert_eq!(status, 400);
        assert!(body.contains("access_list_name_required"), "{body}");
        let (status, body) = send(addr, Method::POST, PATH, &editor, Some(office(None))).await?;
        assert_eq!(status, 400);
        assert!(body.contains("password_required"), "{body}");

        let body = Some(office(Some("alice-secret")));
        let (status, body) = send(addr, Method::POST, PATH, &editor, body).await?;
        assert_eq!(status, 200, "{body}");

        // Every account reads the lists without the password hash.
        let listed = lists(addr, &viewer).await?;
        assert_eq!(listed.len(), 1);
        let id = listed[0]["id"].as_str().unwrap_or_default().to_string();
        let mut masked = office(None);
        masked["id"] = json!(id);
        masked["auth"]["users"][0]["password_set"] = json!(true);
        assert_eq!(listed[0], masked);

        // An update without the password keeps it.
        let item = format!("{PATH}/{id}");
        let mut renamed = office(None);
        renamed["name"] = json!("Head office");
        let (status, body) = send(addr, Method::PUT, &item, &editor, Some(renamed)).await?;
        assert_eq!(status, 200, "{body}");
        let listed = lists(addr, &editor).await?;
        assert_eq!(listed[0]["name"], "Head office");
        assert_eq!(listed[0]["auth"]["users"][0]["password_set"], true);

        let missing = format!("{PATH}/missing");
        let (status, _) = send(addr, Method::PUT, &missing, &editor, Some(office(None))).await?;
        assert_eq!(status, 404);
        let (status, _) = send(addr, Method::DELETE, &item, &scoped, None).await?;
        assert_eq!(status, 403);
        let (status, _) = send(addr, Method::DELETE, &item, &editor, None).await?;
        assert_eq!(status, 200);
        let (status, _) = send(addr, Method::DELETE, &item, &editor, None).await?;
        assert_eq!(status, 404);
        assert!(lists(addr, &editor).await?.is_empty());
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
