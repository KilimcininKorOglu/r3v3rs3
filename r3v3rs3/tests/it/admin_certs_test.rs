use r3v3rs3::{admin::start_admin, certs::Cert, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::cert::{CertInfo, CertKind, SelfSignedCertKind, SelfSignedCertRequest};
use reqwest::header::COOKIE;
use std::{collections::HashMap, net::SocketAddr};
use tracing_subscriber::filter::LevelFilter;

use crate::common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener, with_server};

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
            channels.accounts.clone(),
        ));
        wait_for_listener(addr).await?;
        let cookie = admin_session_cookie(addr).await?;
        let client = reqwest::Client::new();
        request_client_cert(&client, addr, &cookie).await?;
        let certs = list_certs(&client, addr, &cookie).await?;
        check_client_cert_listed(&certs)
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Asks the admin API for a self-signed client certificate for `client.example.com`.
async fn request_client_cert(
    client: &reqwest::Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<()> {
    client
        .post(format!("http://{addr}/api/certs/self_sign"))
        .header(COOKIE, cookie)
        .json(&SelfSignedCertRequest {
            san: vec!["client.example.com".parse()?],
            ca_cert: None,
            kind: SelfSignedCertKind::Client,
        })
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Reads the certificate list from the admin API.
async fn list_certs(
    client: &reqwest::Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<Vec<CertInfo>> {
    Ok(client
        .get(format!("http://{addr}/api/certs"))
        .header(COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Checks that the list holds the root and the client certificate with its key, and no server
/// certificate.
fn check_client_cert_listed(certs: &[CertInfo]) -> anyhow::Result<()> {
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
}

#[tokio::test]
async fn upload_cert_enforces_size_limit_and_rejects_empty() -> anyhow::Result<()> {
    let dir =
        std::env::temp_dir().join(format!("r3v3rs3-admin-certs-upload-{}", std::process::id()));
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
        let cookie = admin_session_cookie(addr).await?;
        check_upload_limits(addr, &cookie).await
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

const BOUNDARY: &str = "---------------------------1234567890";

/// Posts a multipart body to the root certificate upload and returns the status.
async fn upload_root(
    addr: SocketAddr,
    cookie: &str,
    body: Vec<u8>,
) -> anyhow::Result<reqwest::StatusCode> {
    Ok(reqwest::Client::new()
        .post(format!("http://{addr}/api/certs/upload?kind=root"))
        .header(COOKIE, cookie)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .send()
        .await?
        .status())
}

/// A multipart body with one `chain` field.
fn chain_body(chain: &[u8]) -> Vec<u8> {
    let mut body =
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"chain\"\r\n\r\n")
            .into_bytes();
    body.extend_from_slice(chain);
    body.extend(format!("\r\n--{BOUNDARY}--\r\n").into_bytes());
    body
}

/// Checks that an empty upload fails, and that a valid certificate is refused only because its
/// field is larger than 1 MiB: the same certificate with trailing whitespace is a valid PEM
/// document, so only the size limit can refuse it.
async fn check_upload_limits(addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    let empty = format!("--{BOUNDARY}--\r\n").into_bytes();
    assert_eq!(
        upload_root(addr, cookie, empty).await?,
        reqwest::StatusCode::BAD_REQUEST
    );

    let accepted = Cert::new_ca()?;
    let status = upload_root(addr, cookie, chain_body(&accepted.pem_chain)).await?;
    assert_eq!(status, reqwest::StatusCode::OK);

    let oversized = Cert::new_ca()?;
    let mut chain = oversized.pem_chain.clone();
    chain.extend(vec![b'\n'; 3 * 1024 * 1024]);
    // RFC 9110 section 15.5.14: content larger than the server accepts is 413.
    let status = upload_root(addr, cookie, chain_body(&chain)).await?;
    assert_eq!(status, reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    let certs = list_certs(&reqwest::Client::new(), addr, cookie).await?;
    let ids = certs.iter().map(|cert| cert.id).collect::<Vec<_>>();
    assert!(ids.contains(&accepted.id()), "{ids:?}");
    assert!(!ids.contains(&oversized.id()), "{ids:?}");
    Ok(())
}
