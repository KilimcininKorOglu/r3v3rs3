use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::header_rules::HeaderRule;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, TcpProxy};
use r3v3rs3_api::security_headers::{
    AnalyzeHeadersRequest, ApplySecurityProfileRequest, SecurityGrade, SecurityHeadersAudit,
    SecurityProfile, SecurityProfileInfo,
};
use reqwest::header::COOKIE;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::net::SocketAddr;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener, with_server};

/// Sends an authenticated GET to the admin API and decodes the JSON answer.
async fn api_get<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: String,
    cookie: &str,
) -> anyhow::Result<T> {
    Ok(client
        .get(url)
        .header(COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Sends an authenticated POST with a JSON body to the admin API and checks the status.
async fn api_post<B: Serialize>(
    client: &reqwest::Client,
    url: String,
    cookie: &str,
    body: &B,
) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .post(url)
        .header(COOKIE, cookie)
        .json(body)
        .send()
        .await?
        .error_for_status()?)
}

/// Sends an authenticated POST with a JSON body to the admin API and decodes the JSON answer.
async fn api_post_json<B: Serialize, T: DeserializeOwned>(
    client: &reqwest::Client,
    url: String,
    cookie: &str,
    body: &B,
) -> anyhow::Result<T> {
    Ok(api_post(client, url, cookie, body).await?.json().await?)
}

#[tokio::test]
async fn security_headers_profiles_and_analyze_api() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-sec-hdr-api-{}", std::process::id()));
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
        check_profiles_and_analyze(addr, &cookie).await
    })
    .await
}

/// Checks the profile list and the grades of an empty rule set and of the strict profile.
async fn check_profiles_and_analyze(addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // 1. List profiles
    let profiles: Vec<SecurityProfileInfo> = api_get(
        &client,
        format!("http://{addr}/api/security/headers/profiles"),
        cookie,
    )
    .await?;

    assert_eq!(profiles.len(), 4);
    assert!(profiles.iter().any(|p| p.id == SecurityProfile::Strict));
    assert!(profiles.iter().any(|p| p.id == SecurityProfile::Balanced));

    // 2. Analyze empty rules
    let empty_audit: SecurityHeadersAudit = api_post_json(
        &client,
        format!("http://{addr}/api/security/headers/analyze"),
        cookie,
        &AnalyzeHeadersRequest { rules: vec![] },
    )
    .await?;

    assert!(empty_audit.score < 50);
    assert_eq!(empty_audit.grade, SecurityGrade::F);

    // 3. Analyze strict profile rules
    let strict_rules = SecurityProfile::Strict.generate_rules();
    let strict_audit: SecurityHeadersAudit = api_post_json(
        &client,
        format!("http://{addr}/api/security/headers/analyze"),
        cookie,
        &AnalyzeHeadersRequest {
            rules: strict_rules,
        },
    )
    .await?;

    assert_eq!(strict_audit.score, 100);
    assert_eq!(strict_audit.grade, SecurityGrade::APlus);

    Ok(())
}

#[tokio::test]
async fn proxy_security_headers_audit_and_profile_application() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-sec-hdr-proxy-{}", std::process::id()));
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
        check_http_proxy_profile(&client, addr, &cookie).await?;
        check_tcp_proxy_is_refused(&client, addr, &cookie).await
    })
    .await
}

/// Creates a proxy through the admin API and returns the id of the proxy with its name.
async fn create_proxy(
    client: &reqwest::Client,
    addr: SocketAddr,
    cookie: &str,
    proxy: &Proxy,
    expect_msg: &str,
) -> anyhow::Result<ShortId> {
    api_post(client, format!("http://{addr}/api/proxies"), cookie, proxy).await?;

    let proxy_list: Vec<ProxyEntry> =
        api_get(client, format!("http://{addr}/api/proxies"), cookie).await?;
    Ok(proxy_list
        .into_iter()
        .find(|p| p.proxy.name == proxy.name)
        .expect(expect_msg)
        .id)
}

/// Checks that an HTTP proxy grades F first, grades A+ after the Balanced profile, and keeps the
/// header rules of the profile.
async fn check_http_proxy_profile(
    client: &reqwest::Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<()> {
    // Create an HTTP proxy
    let http_proxy = Proxy {
        name: "test-web".to_string(),
        active: true,
        kind: r3v3rs3_api::proxy::ProxyKind::Http(Box::default()),
        ports: vec![],
    };
    let http_id = create_proxy(client, addr, cookie, &http_proxy, "test-web proxy").await?;

    // Audit HTTP proxy before applying profile
    let initial_audit: SecurityHeadersAudit = api_get(
        client,
        format!("http://{addr}/api/proxies/{http_id}/security-headers"),
        cookie,
    )
    .await?;
    assert_eq!(initial_audit.grade, SecurityGrade::F);

    // Apply Balanced profile
    let applied_audit: SecurityHeadersAudit = api_post_json(
        client,
        format!("http://{addr}/api/proxies/{http_id}/security-headers/apply-profile"),
        cookie,
        &ApplySecurityProfileRequest {
            profile: SecurityProfile::Balanced,
        },
    )
    .await?;
    assert_eq!(applied_audit.grade, SecurityGrade::APlus);
    assert!(applied_audit.score >= 80);

    // Fetch proxy and verify header rules were saved
    let fetched: ProxyEntry = api_get(
        client,
        format!("http://{addr}/api/proxies/{http_id}"),
        cookie,
    )
    .await?;
    check_hsts_rule_saved(fetched);
    Ok(())
}

/// Checks that the saved HTTP proxy sets the `Strict-Transport-Security` response header.
fn check_hsts_rule_saved(fetched: ProxyEntry) {
    if let r3v3rs3_api::proxy::ProxyKind::Http(http) = fetched.proxy.kind {
        assert!(!http.headers.response.is_empty());
        assert!(
            http.headers
                .response
                .iter()
                .any(|r| matches!(r, HeaderRule::Set { name, .. } if name.eq_ignore_ascii_case("Strict-Transport-Security")))
        );
    } else {
        panic!("Expected HttpProxy");
    }
}

/// Checks that the security header audit of a TCP proxy answers 400.
async fn check_tcp_proxy_is_refused(
    client: &reqwest::Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<()> {
    // Test with TCP proxy (should return 400 NotHttpProxy)
    let tcp_proxy = Proxy {
        name: "test-tcp".to_string(),
        active: true,
        kind: r3v3rs3_api::proxy::ProxyKind::Tcp(TcpProxy::default()),
        ports: vec![],
    };
    let tcp_id = create_proxy(client, addr, cookie, &tcp_proxy, "test-tcp proxy").await?;

    let err_status = client
        .get(format!(
            "http://{addr}/api/proxies/{tcp_id}/security-headers"
        ))
        .header(COOKIE, cookie)
        .send()
        .await?
        .status();
    assert_eq!(err_status, reqwest::StatusCode::BAD_REQUEST);

    Ok(())
}
