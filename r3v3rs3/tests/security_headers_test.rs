use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::header_rules::HeaderRule;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, TcpProxy};
use r3v3rs3_api::security_headers::{
    AnalyzeHeadersRequest, ApplySecurityProfileRequest, SecurityGrade, SecurityHeadersAudit,
    SecurityProfile, SecurityProfileInfo,
};
use reqwest::header::COOKIE;
use std::collections::HashMap;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener, with_server};

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
        let client = reqwest::Client::new();

        // 1. List profiles
        let profiles: Vec<SecurityProfileInfo> = client
            .get(format!("http://{addr}/api/security/headers/profiles"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        assert_eq!(profiles.len(), 4);
        assert!(profiles.iter().any(|p| p.id == SecurityProfile::Strict));
        assert!(profiles.iter().any(|p| p.id == SecurityProfile::Balanced));

        // 2. Analyze empty rules
        let empty_audit: SecurityHeadersAudit = client
            .post(format!("http://{addr}/api/security/headers/analyze"))
            .header(COOKIE, &cookie)
            .json(&AnalyzeHeadersRequest { rules: vec![] })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        assert!(empty_audit.score < 50);
        assert_eq!(empty_audit.grade, SecurityGrade::F);

        // 3. Analyze strict profile rules
        let strict_rules = SecurityProfile::Strict.generate_rules();
        let strict_audit: SecurityHeadersAudit = client
            .post(format!("http://{addr}/api/security/headers/analyze"))
            .header(COOKIE, &cookie)
            .json(&AnalyzeHeadersRequest {
                rules: strict_rules,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        assert_eq!(strict_audit.score, 100);
        assert_eq!(strict_audit.grade, SecurityGrade::APlus);

        Ok(())
    })
    .await
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

        // Create an HTTP proxy
        let http_proxy = Proxy {
            name: "test-web".to_string(),
            active: true,
            kind: r3v3rs3_api::proxy::ProxyKind::Http(Box::default()),
            ports: vec![],
        };
        client
            .post(format!("http://{addr}/api/proxies"))
            .header(COOKIE, &cookie)
            .json(&http_proxy)
            .send()
            .await?
            .error_for_status()?;

        let proxy_list: Vec<ProxyEntry> = client
            .get(format!("http://{addr}/api/proxies"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let http_id = proxy_list
            .into_iter()
            .find(|p| p.proxy.name == "test-web")
            .expect("test-web proxy")
            .id;

        // Audit HTTP proxy before applying profile
        let initial_audit: SecurityHeadersAudit = client
            .get(format!(
                "http://{addr}/api/proxies/{http_id}/security-headers"
            ))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(initial_audit.grade, SecurityGrade::F);

        // Apply Balanced profile
        let applied_audit: SecurityHeadersAudit = client
            .post(format!(
                "http://{addr}/api/proxies/{http_id}/security-headers/apply-profile"
            ))
            .header(COOKIE, &cookie)
            .json(&ApplySecurityProfileRequest {
                profile: SecurityProfile::Balanced,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(applied_audit.grade, SecurityGrade::APlus);
        assert!(applied_audit.score >= 80);

        // Fetch proxy and verify header rules were saved
        let fetched: ProxyEntry = client
            .get(format!("http://{addr}/api/proxies/{http_id}"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
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

        // Test with TCP proxy (should return 400 NotHttpProxy)
        let tcp_proxy = Proxy {
            name: "test-tcp".to_string(),
            active: true,
            kind: r3v3rs3_api::proxy::ProxyKind::Tcp(TcpProxy::default()),
            ports: vec![],
        };
        client
            .post(format!("http://{addr}/api/proxies"))
            .header(COOKIE, &cookie)
            .json(&tcp_proxy)
            .send()
            .await?
            .error_for_status()?;

        let proxy_list: Vec<ProxyEntry> = client
            .get(format!("http://{addr}/api/proxies"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let tcp_id = proxy_list
            .into_iter()
            .find(|p| p.proxy.name == "test-tcp")
            .expect("test-tcp proxy")
            .id;

        let err_status = client
            .get(format!("http://{addr}/api/proxies/{tcp_id}/security-headers"))
            .header(COOKIE, &cookie)
            .send()
            .await?
            .status();
        assert_eq!(err_status, reqwest::StatusCode::BAD_REQUEST);

        Ok(())
    })
    .await
}
