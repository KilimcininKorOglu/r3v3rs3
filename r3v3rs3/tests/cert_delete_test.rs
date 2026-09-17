use r3v3rs3::certs::Cert;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::cert::MAX_DELETE_CERTS;
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, alloc_tcp_port, send, session_cookie, wait_for_listener, with_server};

#[tokio::test]
async fn a_request_deletes_each_certificate_that_nothing_uses() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-cert-delete-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let first = Cert::new_ca()?;
    let second = Cert::new_ca()?;
    let used = Cert::new_ca()?;
    let first_id = first.id().to_string();
    let second_id = second.id().to_string();
    let used_id = used.id().to_string();
    let mut config = AppConfig::default();
    // A disabled provider still keeps its client certificate in use.
    config.discovery.docker.client_cert = Some(used.id());
    let certs = [first, second, used]
        .into_iter()
        .map(|cert| (cert.id(), Arc::new(cert)))
        .collect::<HashMap<_, _>>();
    let storage = TestStorage::builder()
        .config(config)
        .certs(certs)
        .account("admin", "admin-secret", Role::Admin, None)
        .account("viewer", "viewer-secret", Role::Viewer, None)
        .build();

    let addr = alloc_tcp_port().await?.socket_addr();
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
        let admin = session_cookie(addr, "admin", "admin-secret").await?;
        let viewer = session_cookie(addr, "viewer", "viewer-secret").await?;
        let delete = "/api/certs/delete";

        // The second copy of an id gets no result of its own.
        let ids = json!({"ids": [first_id, used_id, "abc", first_id, second_id]});
        let (status, _) = send(addr, Method::POST, delete, &viewer, Some(ids.clone())).await?;
        assert_eq!(status, 403);

        let (status, body) = send(addr, Method::POST, delete, &admin, Some(ids)).await?;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body)?,
            json!([
                {"id": first_id, "status": "deleted"},
                {"id": used_id, "status": "in_use"},
                {"id": "abc", "status": "not_found"},
                {"id": second_id, "status": "deleted"},
            ])
        );
        let (_, body) = send(addr, Method::GET, "/api/certs", &admin, None).await?;
        let listed = serde_json::from_str::<Vec<Value>>(&body)?
            .into_iter()
            .map(|cert| cert["id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(listed, [json!(used_id)]);

        let too_many = json!({"ids": vec![used_id.clone(); MAX_DELETE_CERTS + 1]});
        let (status, body) = send(addr, Method::POST, delete, &admin, Some(too_many)).await?;
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("too_many_certificates"), "{body}");
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
