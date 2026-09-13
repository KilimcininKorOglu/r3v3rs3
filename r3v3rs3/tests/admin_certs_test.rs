use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::cert::{CertInfo, CertKind, SelfSignedCertKind, SelfSignedCertRequest};
use reqwest::header::COOKIE;
use std::collections::HashMap;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{admin_session_cookie, alloc_tcp_port, wait_for_listener, with_server, TestStorage};

#[tokio::test]
async fn self_sign_creates_a_client_certificate() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-admin-certs-{}", std::process::id()));
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
        ));
        wait_for_listener(addr).await?;
        let cookie = admin_session_cookie(addr).await?;
        let client = reqwest::Client::new();

        client
            .post(format!("http://{addr}/api/certs/self_sign"))
            .header(COOKIE, &cookie)
            .json(&SelfSignedCertRequest {
                san: vec!["client.example.com".parse()?],
                ca_cert: None,
                kind: SelfSignedCertKind::Client,
            })
            .send()
            .await?
            .error_for_status()?;

        let certs: Vec<CertInfo> = client
            .get(format!("http://{addr}/api/certs"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let kinds = certs.iter().map(|cert| cert.kind).collect::<Vec<_>>();
        assert!(kinds.contains(&CertKind::Root), "kinds: {kinds:?}");
        assert!(!kinds.contains(&CertKind::Server), "kinds: {kinds:?}");

        let cert = certs
            .iter()
            .find(|cert| cert.kind == CertKind::Client)
            .ok_or_else(|| anyhow::anyhow!("no client certificate in {kinds:?}"))?;
        assert!(cert.has_private_key);
        assert_eq!(cert.san, vec!["client.example.com".parse()?]);
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
