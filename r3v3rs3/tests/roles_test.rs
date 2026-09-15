use r3v3rs3::{
    accounts::Caller,
    admin::start_admin,
    config::new_appinfo,
    log::DatabaseLayer,
    server::rpc::{config::GetConfig, ports::AddPort, proxies::AddProxy},
};
use r3v3rs3_api::{auth::Role, error::Error, id::ShortId, proxy::Proxy};
use reqwest::{header::COOKIE, Client, Method};
use std::{collections::BTreeSet, net::SocketAddr};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    alloc_tcp_port, call_as, port_entry, session_cookie, wait_for_listener, with_server,
    TestStorage,
};

fn caller(role: Role, proxies: Option<BTreeSet<ShortId>>) -> Caller {
    Caller {
        username: "user".to_string(),
        role,
        proxies,
    }
}

fn web_only() -> Option<BTreeSet<ShortId>> {
    Some(BTreeSet::from(["web".parse().unwrap()]))
}

#[tokio::test]
async fn the_server_runs_only_the_methods_that_the_role_allows() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();
    with_server(storage, |mut channels| async move {
        let port = || port_entry("unused", "/ip4/127.0.0.1/tcp/0".parse().unwrap()).port;

        let viewer = caller(Role::Viewer, None);
        let entry = Proxy::default();
        let result = call_as(&mut channels, viewer, AddProxy { entry }).await?;
        assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");

        let restricted = caller(Role::Editor, web_only());
        let result = call_as(&mut channels, restricted, AddPort { entry: port() }).await?;
        assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");

        let editor = caller(Role::Editor, None);
        let result = call_as(&mut channels, editor.clone(), GetConfig).await?;
        assert!(matches!(result, Err(Error::Forbidden)));
        let result = call_as(&mut channels, editor, AddPort { entry: port() }).await?;
        assert!(result.is_ok(), "{result:?}");
        Ok(())
    })
    .await
}

async fn status(
    client: &Client,
    addr: SocketAddr,
    method: Method,
    path: &str,
    cookie: &str,
) -> anyhow::Result<u16> {
    let port = port_entry("unused", "/ip4/127.0.0.1/tcp/0".parse()?).port;
    let res = client
        .request(method, format!("http://{addr}{path}"))
        .header(COOKIE, cookie)
        .json(&port)
        .send()
        .await?;
    Ok(res.status().as_u16())
}

#[tokio::test]
async fn the_admin_api_answers_403_to_an_action_that_the_role_does_not_allow() -> anyhow::Result<()>
{
    let dir = std::env::temp_dir().join(format!("r3v3rs3-roles-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .account("viewer", "viewer-secret", Role::Viewer, None)
        .account("editor", "editor-secret", Role::Editor, web_only())
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

        let viewer = session_cookie(addr, "viewer", "viewer-secret").await?;
        let checks = [
            (Method::GET, "/api/ports", 200),
            (Method::POST, "/api/ports", 403),
            (Method::GET, "/api/config", 403),
            (Method::POST, "/api/cdn/refresh", 403),
            (Method::GET, "/api/certs/abc/download", 403),
        ];
        for (method, path, expected) in checks {
            let actual = status(&client, addr, method.clone(), path, &viewer).await?;
            assert_eq!(actual, expected, "viewer {method} {path}");
        }

        let editor = session_cookie(addr, "editor", "editor-secret").await?;
        let checks = [
            (Method::GET, "/api/certs", 200),
            (Method::POST, "/api/ports", 403),
            (Method::GET, "/api/config", 403),
        ];
        for (method, path, expected) in checks {
            let actual = status(&client, addr, method.clone(), path, &editor).await?;
            assert_eq!(actual, expected, "editor {method} {path}");
        }
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
