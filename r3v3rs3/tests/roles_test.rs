use r3v3rs3::{
    accounts::Caller,
    admin::start_admin,
    cluster::layout::StateKind,
    command::ServerCommand,
    config::{new_appinfo, storage::Storage},
    log::DatabaseLayer,
    server::rpc::{
        config::GetConfig,
        ports::AddPort,
        proxies::{AddProxy, DeleteProxy, GetProxy, GetProxyList, UpdateProxy},
        ErasedRpcMethod, RpcMethod, RpcWrapper,
    },
};
use r3v3rs3_api::{
    auth::Role,
    error::Error,
    id::ShortId,
    proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind},
};
use reqwest::{header::COOKIE, Client, Method, Response};
use std::{collections::BTreeSet, net::SocketAddr, time::Duration};
use tokio::sync::mpsc;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    alloc_tcp_port, call, call_as, http_route, port_entry, session_cookie, wait_for_listener,
    with_server, TestStorage,
};

fn caller(role: Role, proxies: Option<BTreeSet<ShortId>>) -> Caller {
    Caller {
        username: "editor".to_string(),
        role,
        proxies,
        client: None,
    }
}

fn web_only() -> Option<BTreeSet<ShortId>> {
    Some(BTreeSet::from(["web".parse().unwrap()]))
}

fn http_proxy(upstream: &str) -> Proxy {
    Proxy {
        kind: ProxyKind::Http(Box::new(HttpProxy {
            routes: vec![http_route("/", upstream, None)],
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn proxy_entry(id: &str, upstream: &str) -> ProxyEntry {
    ProxyEntry {
        id: id.parse().unwrap(),
        proxy: http_proxy(upstream),
        source: None,
    }
}

/// Storage with the proxies `web` and `other`, and an editor that sees only `web`.
fn restricted_storage() -> TestStorage {
    TestStorage::builder()
        .proxies(vec![
            proxy_entry("web", "http://127.0.0.1:1/"),
            proxy_entry("other", "http://127.0.0.1:1/"),
        ])
        .account("editor", "editor-secret", Role::Editor, web_only())
        .build()
}

/// A directory with a log database for the admin API.
async fn log_dir(name: &str) -> anyhow::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    Ok(dir)
}

#[tokio::test]
async fn the_server_runs_only_the_methods_that_the_role_allows() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();
    with_server(storage, |mut channels| async move {
        let port = || port_entry("unused", "/ip4/127.0.0.1/tcp/0".parse().unwrap()).port;

        let viewer = caller(Role::Viewer, None);
        let add = AddProxy {
            entry: Proxy::default(),
            owner: None,
        };
        let result = call_as(&mut channels, viewer, add).await?;
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

#[tokio::test]
async fn an_editor_with_a_proxy_list_sees_and_changes_only_its_proxies() -> anyhow::Result<()> {
    let storage = restricted_storage();
    with_server(storage.clone(), |mut channels| async move {
        let editor = caller(Role::Editor, web_only());
        let web: ShortId = "web".parse()?;
        let other: ShortId = "other".parse()?;

        let proxies = call_as(&mut channels, editor.clone(), GetProxyList).await??;
        let ids = proxies.iter().map(|entry| entry.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![web]);

        let result = call_as(&mut channels, editor.clone(), GetProxy { id: other }).await?;
        assert!(matches!(result, Err(Error::IdNotFound { .. })));
        let entry = proxy_entry("other", "http://127.0.0.1:2/");
        let result = call_as(&mut channels, editor.clone(), UpdateProxy { entry }).await?;
        assert!(matches!(result, Err(Error::IdNotFound { .. })));
        let result = call_as(&mut channels, editor.clone(), DeleteProxy { id: other }).await?;
        assert!(matches!(result, Err(Error::IdNotFound { .. })));

        let add = AddProxy {
            entry: http_proxy("http://127.0.0.1:3/"),
            owner: Some("editor".to_string()),
        };
        call_as(&mut channels, editor, add).await??;
        let granted = storage.load_accounts().await?["editor"].proxies.clone();
        let created = granted
            .iter()
            .flatten()
            .copied()
            .find(|id| *id != web)
            .ok_or_else(|| anyhow::anyhow!("the new proxy is not in the account list"))?;
        let directory = channels.accounts.borrow().clone();
        assert_eq!(directory.get("editor").map(|a| &a.proxies), Some(&granted));

        // The admin API builds the caller from the directory for each request.
        let editor = caller(Role::Editor, granted.clone());
        let proxies = call_as(&mut channels, editor, GetProxyList).await??;
        let mut ids = proxies.iter().map(|entry| entry.id).collect::<Vec<_>>();
        ids.sort();
        let mut expected = vec![web, created];
        expected.sort();
        assert_eq!(ids, expected);

        call(&mut channels, DeleteProxy { id: created }).await??;
        assert_eq!(storage.load_accounts().await?["editor"].proxies, web_only());
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
    let dir = log_dir("roles").await?;
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

/// Sends a method as the admin API itself. Nothing waits for the result.
async fn send_system<M: RpcMethod + 'static>(
    command: &mpsc::Sender<ServerCommand>,
    method: M,
) -> anyhow::Result<()> {
    let arg = Box::new(RpcWrapper::new(method)) as Box<dyn ErasedRpcMethod>;
    let caller = Caller::system();
    command
        .send(ServerCommand::CallMethod {
            id: usize::MAX,
            arg,
            caller,
        })
        .await?;
    Ok(())
}

/// Reads the body until the duration passes or the body ends.
async fn read_for(response: &mut Response, duration: Duration) -> anyhow::Result<String> {
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + duration;
    while let Ok(chunk) = tokio::time::timeout_at(deadline, response.chunk()).await {
        let Some(bytes) = chunk? else {
            break;
        };
        text.push_str(&String::from_utf8_lossy(&bytes));
    }
    Ok(text)
}

#[tokio::test]
async fn the_event_stream_holds_only_the_proxies_of_the_account_and_ends_when_the_account_changes(
) -> anyhow::Result<()> {
    let dir = log_dir("roles-events").await?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = restricted_storage();

    let app_info = new_appinfo(&dir, &dir);
    with_server(storage.clone(), |channels| async move {
        let command = channels.command.clone();
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
        let cookie = session_cookie(addr, "editor", "editor-secret").await?;

        let logs = status(&client, addr, Method::GET, "/api/logs/other", &cookie).await?;
        assert_eq!(logs, 404);

        let mut events = client
            .get(format!("http://{addr}/api/events"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?;
        let entry = proxy_entry("other", "http://127.0.0.1:2/");
        send_system(&command, UpdateProxy { entry }).await?;
        let text = read_for(&mut events, Duration::from_secs(1)).await?;
        assert!(text.contains("proxies_updated"), "{text}");
        assert!(text.contains("\"web\""), "{text}");
        assert!(!text.contains("\"other\""), "{text}");

        let mut accounts = storage.load_accounts().await?;
        accounts.remove("editor");
        storage.save_accounts(&accounts).await?;
        let kinds = vec![StateKind::Accounts];
        command
            .send(ServerCommand::ClusterChanged { kinds })
            .await?;
        let rest = tokio::time::timeout(Duration::from_secs(5), async {
            while events.chunk().await?.is_some() {}
            anyhow::Ok(())
        });
        rest.await
            .map_err(|_| anyhow::anyhow!("the event stream did not end"))??;
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
