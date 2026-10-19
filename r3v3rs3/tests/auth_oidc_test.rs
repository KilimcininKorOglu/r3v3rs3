use r3v3rs3_api::{
    policy::{AuthPolicy, OidcAuth},
    proxy::HttpProxy,
};
use reqwest::{
    StatusCode,
    header::{COOKIE, LOCATION, SET_COOKIE},
    redirect::Policy,
};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn fake_jwt(claims_json: &str) -> String {
    use base64::Engine;
    let b64 = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = b64.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = b64.encode(claims_json);
    format!("{header}.{payload}.invalidsig")
}

fn oidc_proxy(
    upstream: &str,
    auth_url: String,
    token_url: String,
    allowed_emails: Vec<String>,
) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        upgrade_insecure: false,
        auth: AuthPolicy::Oidc(Box::new(OidcAuth {
            issuer_url: String::new(),
            authorization_url: auth_url,
            token_url,
            userinfo_url: String::new(),
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
            client_secret_set: true,
            scopes: vec!["openid".into(), "email".into()],
            allowed_emails,
        })),
        ..Default::default()
    }
}

#[tokio::test]
async fn oidc_auth_flow_success_and_logout() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut oidc_server = mockito::Server::new_async().await;
    let mut upstream = mockito::Server::new_async().await;

    let auth_endpoint = format!("{}/oauth/authorize", oidc_server.url());
    let token_endpoint = format!("{}/oauth/token", oidc_server.url());

    let id_token = fake_jwt(r#"{"email":"alice@example.com","sub":"sub123"}"#);
    let token_body = serde_json::json!({
        "access_token": "mock_access_token",
        "id_token": id_token,
        "token_type": "Bearer"
    })
    .to_string();

    let _mock_token = oidc_server
        .mock("POST", "/oauth/token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(token_body)
        .expect(1)
        .create_async()
        .await;

    let mock_upstream = upstream
        .mock("GET", "/protected")
        .match_header("x-forwarded-user", "alice@example.com")
        .match_header("x-forwarded-email", "alice@example.com")
        .with_body("upstream response ok")
        .expect(1)
        .create_async()
        .await;

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("oidc", &port)])
        .proxies(vec![http_proxy_entry(
            "oidc",
            "oidc",
            oidc_proxy(
                &upstream.url(),
                auth_endpoint.clone(),
                token_endpoint,
                vec!["alice@example.com".into()],
            ),
        )])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;

        // 1. Unauthenticated request to /protected redirects to OIDC login
        let resp = client.get(port.http_url("/protected")).send().await?;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp.headers().get(LOCATION).unwrap().to_str()?;
        assert_eq!(loc, "/.r3v3rs3/auth/oidc/login?redirect=%2Fprotected");

        // 2. Client navigates to OIDC login
        let resp = client
            .get(port.http_url("/.r3v3rs3/auth/oidc/login?redirect=%2Fprotected"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let auth_redirect = resp.headers().get(LOCATION).unwrap().to_str()?;
        assert!(auth_redirect.starts_with(&auth_endpoint));
        assert!(auth_redirect.contains("client_id=test-client-id"));

        let state_cookie_header = resp
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                if s.starts_with("r3v3rs3_oidc_state=") {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .expect("state cookie must be set");

        let state_val = state_cookie_header
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("r3v3rs3_oidc_state=");
        let (state_token, _) = state_val.split_once(':').unwrap();

        // 3. Provider redirects back to callback with state & code
        let callback_url = format!(
            "/.r3v3rs3/auth/oidc/callback?code=mock_code_123&state={}",
            state_token
        );
        let resp = client
            .get(port.http_url(&callback_url))
            .header(COOKIE, format!("r3v3rs3_oidc_state={state_val}"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            resp.headers().get(LOCATION).unwrap().to_str()?,
            "/protected"
        );

        let session_cookie = resp
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                if s.starts_with("r3v3rs3_oidc=") && !s.contains("Max-Age=0") {
                    Some(s.split(';').next().unwrap().to_string())
                } else {
                    None
                }
            })
            .expect("session cookie must be set");

        // 4. Client accesses /protected with session cookie
        let resp = client
            .get(port.http_url("/protected"))
            .header(COOKIE, &session_cookie)
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "upstream response ok");
        mock_upstream.assert_async().await;

        // 5. Logout
        let resp = client
            .get(port.http_url("/.r3v3rs3/auth/oidc/logout"))
            .header(COOKIE, &session_cookie)
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FOUND);

        // 6. Accessing /protected again redirects to login
        let resp = client.get(port.http_url("/protected")).send().await?;
        assert_eq!(resp.status(), StatusCode::FOUND);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn oidc_auth_flow_disallows_unauthorized_email() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut oidc_server = mockito::Server::new_async().await;
    let upstream = mockito::Server::new_async().await;

    let auth_endpoint = format!("{}/oauth/authorize", oidc_server.url());
    let token_endpoint = format!("{}/oauth/token", oidc_server.url());

    let id_token = fake_jwt(r#"{"email":"stranger@other.com","sub":"sub456"}"#);
    let token_body = serde_json::json!({
        "access_token": "mock_access_token_2",
        "id_token": id_token,
        "token_type": "Bearer"
    })
    .to_string();

    let _mock_token = oidc_server
        .mock("POST", "/oauth/token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(token_body)
        .expect(1)
        .create_async()
        .await;

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("oidc2", &port)])
        .proxies(vec![http_proxy_entry(
            "oidc2",
            "oidc2",
            oidc_proxy(
                &upstream.url(),
                auth_endpoint,
                token_endpoint,
                vec!["*@company.com".into()],
            ),
        )])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;

        let resp = client
            .get(port.http_url("/.r3v3rs3/auth/oidc/login?redirect=%2Fdashboard"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FOUND);

        let state_cookie_header = resp
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                if s.starts_with("r3v3rs3_oidc_state=") {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .expect("state cookie must be set");

        let state_val = state_cookie_header
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("r3v3rs3_oidc_state=");
        let (state_token, _) = state_val.split_once(':').unwrap();

        let callback_url = format!(
            "/.r3v3rs3/auth/oidc/callback?code=mock_code_bad&state={}",
            state_token
        );
        let resp = client
            .get(port.http_url(&callback_url))
            .header(COOKIE, format!("r3v3rs3_oidc_state={state_val}"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            resp.text()
                .await?
                .contains("not authorized to access this service")
        );

        Ok(())
    })
    .await
}
