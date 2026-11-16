use r3v3rs3_api::{
    policy::{AuthPolicy, OidcAuth},
    proxy::HttpProxy,
};
use reqwest::{
    StatusCode,
    header::{COOKIE, SET_COOKIE},
    redirect::Policy,
};

use crate::common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn make_test_jwt(claims_json: &str, secret: &str) -> String {
    use base64::Engine;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    let b64 = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = b64.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = b64.encode(claims_json);
    let message = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(message.as_bytes());
    let sig = b64.encode(mac.finalize().into_bytes());
    format!("{message}.{sig}")
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

async fn do_oidc_login(
    client: &reqwest::Client,
    base_url: &str,
) -> anyhow::Result<(String, String)> {
    let resp = client
        .get(format!(
            "{base_url}/.r3v3rs3/auth/oidc/login?redirect=%2Fprotected"
        ))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::FOUND);
    let cookie_hdr = resp
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
    let state_val = cookie_hdr
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("r3v3rs3_oidc_state=");
    let (state_token, _) = state_val.split_once(':').unwrap();
    Ok((state_val.to_string(), state_token.to_string()))
}

async fn do_oidc_callback(
    client: &reqwest::Client,
    base_url: &str,
    state_val: &str,
    state_token: &str,
) -> anyhow::Result<String> {
    let callback_url =
        format!("{base_url}/.r3v3rs3/auth/oidc/callback?code=mock_code_123&state={state_token}");
    let resp = client
        .get(&callback_url)
        .header(COOKIE, format!("r3v3rs3_oidc_state={state_val}"))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
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
    Ok(session_cookie)
}

#[tokio::test]
async fn oidc_auth_flow_success_and_logout() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut oidc_server = mockito::Server::new_async().await;
    let mut upstream = mockito::Server::new_async().await;

    let auth_endpoint = format!("{}/oauth/authorize", oidc_server.url());
    let token_endpoint = format!("{}/oauth/token", oidc_server.url());

    let id_token = make_test_jwt(
        r#"{"email":"alice@example.com","sub":"sub123","aud":"test-client-id","exp":9999999999}"#,
        "test-client-secret",
    );
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
        .expect(2)
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
        verify_oidc_flow_and_logout(&client, &port, mock_upstream).await
    })
    .await
}

async fn verify_oidc_flow_and_logout(
    client: &reqwest::Client,
    port: &common::TestPort,
    mock_upstream: mockito::Mock,
) -> anyhow::Result<()> {
    let resp = client.get(port.http_url("/protected")).send().await?;
    assert_eq!(resp.status(), StatusCode::FOUND);

    let (state_val, state_token) = do_oidc_login(client, port.http_url("").as_str()).await?;
    let session_cookie =
        do_oidc_callback(client, port.http_url("").as_str(), &state_val, &state_token).await?;

    let resp = client
        .get(port.http_url("/protected"))
        .header(COOKIE, &session_cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await?, "upstream response ok");

    // The identity headers come from the session only: the upstream mock matches only when
    // every value is alice, so a forged value that reaches it fails the request.
    let resp = client
        .get(port.http_url("/protected"))
        .header(COOKIE, &session_cookie)
        .header("x-forwarded-user", "mallory@evil.test")
        .header("x-forwarded-email", "mallory@evil.test")
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    mock_upstream.assert_async().await;
    check_logout(client, port, &session_cookie).await
}

/// Checks that logout needs a same-site POST and ends the session: the old cookie no longer
/// reaches the upstream (OWASP Session Management, session termination).
async fn check_logout(
    client: &reqwest::Client,
    port: &common::TestPort,
    session_cookie: &str,
) -> anyhow::Result<()> {
    let resp = client
        .get(port.http_url("/.r3v3rs3/auth/oidc/logout"))
        .header(COOKIE, session_cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);

    let resp_cross = client
        .post(port.http_url("/.r3v3rs3/auth/oidc/logout"))
        .header(COOKIE, session_cookie)
        .header("sec-fetch-site", "cross-site")
        .send()
        .await?;
    assert_eq!(resp_cross.status(), StatusCode::FORBIDDEN);

    let resp_ok = client
        .post(port.http_url("/.r3v3rs3/auth/oidc/logout"))
        .header(COOKIE, session_cookie)
        .send()
        .await?;
    assert_eq!(resp_ok.status(), StatusCode::FOUND);

    let resp = client
        .get(port.http_url("/protected"))
        .header(COOKIE, session_cookie)
        .send()
        .await?;
    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "the old session still works"
    );
    Ok(())
}

#[tokio::test]
async fn oidc_auth_flow_disallows_unauthorized_email() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut oidc_server = mockito::Server::new_async().await;
    let upstream = mockito::Server::new_async().await;

    let auth_endpoint = format!("{}/oauth/authorize", oidc_server.url());
    let token_endpoint = format!("{}/oauth/token", oidc_server.url());

    let id_token = make_test_jwt(
        r#"{"email":"stranger@other.com","sub":"sub456","aud":"test-client-id","exp":9999999999}"#,
        "test-client-secret",
    );
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

/// A mock identity provider whose token endpoint answers `expected` times with an id_token of
/// `claims` signed with `secret`. Returns the server, the token mock and the authorization and
/// token URLs.
async fn token_server(
    claims: &str,
    secret: &str,
    expected: usize,
) -> (mockito::ServerGuard, mockito::Mock, String, String) {
    let mut server = mockito::Server::new_async().await;
    let body = serde_json::json!({
        "access_token": "mock_access_token",
        "id_token": make_test_jwt(claims, secret),
        "token_type": "Bearer"
    })
    .to_string();
    let mock = server
        .mock("POST", "/oauth/token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(expected)
        .create_async()
        .await;
    let auth = format!("{}/oauth/authorize", server.url());
    let token = format!("{}/oauth/token", server.url());
    (server, mock, auth, token)
}

/// A storage with one OIDC proxy on the port that admits alice@example.com.
fn oidc_storage(port: &common::TestPort, auth: String, token: String) -> TestStorage {
    TestStorage::builder()
        .ports(vec![http_port_entry("oidc3", port)])
        .proxies(vec![http_proxy_entry(
            "oidc3",
            "oidc3",
            oidc_proxy(
                "http://127.0.0.1:1",
                auth,
                token,
                vec!["alice@example.com".into()],
            ),
        )])
        .build()
}

/// Whether the response sets a session cookie of the proxy.
fn sets_session(resp: &reqwest::Response) -> bool {
    resp.headers().get_all(SET_COOKIE).iter().any(|value| {
        value
            .to_str()
            .is_ok_and(|s| s.starts_with("r3v3rs3_oidc=") && !s.contains("Max-Age=0"))
    })
}

/// Runs the login and the callback with an id_token of `claims` signed with `secret`, and checks
/// that the proxy refuses it: an error status and no session cookie (OIDC Core 3.1.3.7).
async fn assert_token_refused(claims: &str, secret: &str) -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let (_server, _mock, auth, token) = token_server(claims, secret, 1).await;
    let claims = claims.to_string();
    with_server(oidc_storage(&port, auth, token), |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let base = port.http_url("");
        let (state_val, state_token) = do_oidc_login(&client, base.as_str()).await?;
        let resp = client
            .get(format!(
                "{base}/.r3v3rs3/auth/oidc/callback?code=mock_code_123&state={state_token}"
            ))
            .header(COOKIE, format!("r3v3rs3_oidc_state={state_val}"))
            .send()
            .await?;
        let status = resp.status();
        assert!(
            status.is_client_error() || status.is_server_error(),
            "{status}"
        );
        assert!(!sets_session(&resp), "{claims} got a session");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn oidc_auth_flow_rejects_forged_or_expired_tokens() -> anyhow::Result<()> {
    let claims = |exp: u64| {
        format!(
            r#"{{"email":"alice@example.com","sub":"sub123","aud":"test-client-id","exp":{exp}}}"#
        )
    };
    assert_token_refused(&claims(9_999_999_999), "attacker-secret").await?;
    // Expired in 2001, signed with the right secret.
    assert_token_refused(&claims(1_000_000_000), "test-client-secret").await
}

/// OIDC Core 3.1.3.7 item 3: an id_token issued to another client is refused, even when its
/// signature is valid.
#[tokio::test]
async fn oidc_auth_flow_rejects_a_token_for_another_client() -> anyhow::Result<()> {
    assert_token_refused(
        r#"{"email":"alice@example.com","sub":"sub123","aud":"other-client","exp":9999999999}"#,
        "test-client-secret",
    )
    .await
}

/// OIDC Core 3.1.2.7 and RFC 6749 10.12: a callback whose state does not match the state cookie
/// of the browser, or that comes without the cookie, is refused before the code is exchanged.
#[tokio::test]
async fn oidc_callback_rejects_a_wrong_or_missing_state() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let claims =
        r#"{"email":"alice@example.com","sub":"sub123","aud":"test-client-id","exp":9999999999}"#;
    let (_server, token_mock, auth, token) = token_server(claims, "test-client-secret", 0).await;
    with_server(oidc_storage(&port, auth, token), |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let base = port.http_url("");
        let (state_val, state_token) = do_oidc_login(&client, base.as_str()).await?;
        let callback = format!("{base}/.r3v3rs3/auth/oidc/callback?code=mock_code_123&state=");
        let attempts = [
            (format!("{callback}attacker-state"), Some(state_val.clone())),
            (format!("{callback}{state_token}"), None),
        ];
        for (url, cookie) in attempts {
            let mut request = client.get(&url);
            if let Some(cookie) = cookie {
                request = request.header(COOKIE, format!("r3v3rs3_oidc_state={cookie}"));
            }
            let resp = request.send().await?;
            assert!(resp.status().is_client_error(), "{url}: {}", resp.status());
            assert!(!sets_session(&resp), "{url} got a session");
        }
        token_mock.assert_async().await;
        Ok(())
    })
    .await
}

fn extract_set_cookie(resp: &reqwest::Response, prefix: &str) -> String {
    for val in resp.headers().get_all(SET_COOKIE) {
        if let Ok(s) = val.to_str()
            && s.starts_with(prefix)
            && (!prefix.ends_with('=') || !s.contains("Max-Age=0"))
        {
            return s.to_string();
        }
    }
    panic!("cookie {prefix} not found");
}

#[tokio::test]
async fn oidc_cookie_scoped_to_base_path_and_stripped_on_unprotected_route() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut oidc_server = mockito::Server::new_async().await;
    let mut upstream = mockito::Server::new_async().await;

    let auth_endpoint = format!("{}/oauth/authorize", oidc_server.url());
    let token_endpoint = format!("{}/oauth/token", oidc_server.url());

    let id_token = make_test_jwt(
        r#"{"email":"bob@example.com","sub":"sub456","aud":"test-client-id","exp":9999999999}"#,
        "test-client-secret",
    );
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

    let mock_public = upstream
        .mock("GET", "/hello")
        .match_header("cookie", "legit=true")
        .with_body("public response ok")
        .expect(1)
        .create_async()
        .await;

    let mut internal_route = http_route("/internal", &upstream.url(), None);
    internal_route.auth = Some(AuthPolicy::Oidc(Box::new(OidcAuth {
        issuer_url: String::new(),
        authorization_url: auth_endpoint,
        token_url: token_endpoint,
        userinfo_url: String::new(),
        client_id: "test-client-id".into(),
        client_secret: "test-client-secret".into(),
        client_secret_set: true,
        scopes: vec!["openid".into(), "email".into()],
        allowed_emails: vec!["bob@example.com".into()],
    })));

    let public_route = http_route("/public", &upstream.url(), None);

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![internal_route, public_route],
        ..Default::default()
    };

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("oidcscp", &port)])
        .proxies(vec![http_proxy_entry("oidcscp", "oidcscp", proxy)])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;

        let login_url = format!(
            "{}/internal/.r3v3rs3/auth/oidc/login?redirect=%2Finternal%2Fsecret",
            port.http_url("")
        );
        let resp = client.get(&login_url).send().await?;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let state_hdr = extract_set_cookie(&resp, "r3v3rs3_oidc_state=");
        assert!(
            state_hdr.contains("Path=/internal"),
            "state cookie must be scoped: {state_hdr}"
        );

        let state_val = state_hdr
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("r3v3rs3_oidc_state=");
        let (state_token, _) = state_val.split_once(':').unwrap();

        let callback_url = format!(
            "{}/internal/.r3v3rs3/auth/oidc/callback?code=mock_code_123&state={state_token}",
            port.http_url("")
        );
        let resp = client
            .get(&callback_url)
            .header(COOKIE, format!("r3v3rs3_oidc_state={state_val}"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let session_hdr = extract_set_cookie(&resp, "r3v3rs3_oidc=");
        assert!(
            session_hdr.contains("Path=/internal"),
            "session cookie must be scoped: {session_hdr}"
        );

        let resp = client
            .get(port.http_url("/public/hello"))
            .header(COOKIE, "r3v3rs3_oidc=leaked_proxy_token; legit=true")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "public response ok");
        mock_public.assert_async().await;

        Ok(())
    })
    .await
}
