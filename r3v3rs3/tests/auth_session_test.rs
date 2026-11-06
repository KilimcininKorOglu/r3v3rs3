use r3v3rs3_api::{
    app::AppConfig, auth::Role, i18n::Locale, id::ShortId, policy::AuthPolicy, proxy::HttpProxy,
};
use reqwest::{
    StatusCode,
    header::{COOKIE, HOST, LOCATION, SET_COOKIE},
    redirect::Policy,
};
use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

mod common;
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route, with_server,
};

#[tokio::test]
async fn session_auth_signs_clients_in_with_panel_accounts() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;
    let mock_private = upstream
        .mock("GET", "/private?page=1")
        .match_header("cookie", "theme=dark")
        .with_body("private")
        .expect(1)
        .create_async()
        .await;

    let mut config = AppConfig::default();
    config.admin.max_login_attempts = 2;
    config.admin.login_attempts_reset = Duration::from_secs(60);

    let storage = TestStorage::builder()
        .config(config)
        .accounts(HashMap::from([
            ("admin".to_string(), "secret".to_string()),
            ("other".to_string(), "secret".to_string()),
        ]))
        .ports(vec![http_port_entry("session", &port)])
        .proxies(vec![http_proxy_entry(
            "proxy1",
            "session",
            HttpProxy {
                routes: vec![http_route("/", &upstream.url(), None)],
                upgrade_insecure: false,
                auth: AuthPolicy::Session,
                ..Default::default()
            },
        )])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let login_url = port.http_url("/.r3v3rs3/auth/login");

        let resp = client.get(port.http_url("/private?page=1")).send().await?;
        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(
            resp.headers()[LOCATION],
            "/.r3v3rs3/auth/login?redirect=%2Fprivate%3Fpage%3D1"
        );

        let resp = client.post(port.http_url("/private")).send().await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp = client.get(login_url.clone()).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.text().await?;
        assert!(body.contains(r#"name="password""#));
        assert!(body.contains(r#"<html lang="en">"#), "{body}");

        // The sign-in page uses the language and the theme that the WebUI stores in cookies.
        let resp = client
            .get(login_url.clone())
            .header(COOKIE, "r3v3rs3_lang=tr; r3v3rs3_theme=light")
            .send()
            .await?;
        let body = resp.text().await?;
        assert!(
            body.contains(r#"<html lang="tr" data-theme="light">"#),
            "{body}"
        );
        assert!(body.contains(&format!("<h1>{}</h1>", Locale::Tr.t("login.title"))));

        // Failed sign-ins are limited for each client IP address and username.
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let resp = client
                .post(login_url.clone())
                .form(&[("username", "other"), ("password", "wrong")])
                .send()
                .await?;
            assert_eq!(resp.status(), status);
        }

        let resp = client
            .post(login_url.clone())
            .header(COOKIE, "r3v3rs3_lang=tr")
            .form(&[("username", "admin"), ("password", "wrong")])
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.text()
                .await?
                .contains(Locale::Tr.t("login.invalid_credentials"))
        );

        let resp = client
            .post(login_url.clone())
            .form(&[
                ("username", "admin"),
                ("password", "secret"),
                ("redirect", "/private?page=1"),
            ])
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers()[LOCATION], "/private?page=1");
        let set_cookie = resp.headers()[SET_COOKIE].to_str()?;
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Lax"));
        assert!(!set_cookie.contains("Domain"));
        let session = set_cookie.split(';').next().unwrap_or_default().to_string();

        // The upstream server receives the other cookies without the session cookie.
        let resp = client
            .get(port.http_url("/private?page=1"))
            .header(COOKIE, format!("theme=dark; {session}"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "private");

        // The session is valid only on the host where the client signed in.
        let resp = client
            .get(port.http_url("/private?page=1"))
            .header(HOST, "other.example")
            .header(COOKIE, session.clone())
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FOUND);

        // A redirect to another site is replaced with the root path.
        let resp = client
            .post(login_url.clone())
            .form(&[
                ("username", "admin"),
                ("password", "secret"),
                ("redirect", "//evil.example/"),
            ])
            .send()
            .await?;
        assert_eq!(resp.headers()[LOCATION], "/");

        let resp = client
            .post(port.http_url("/.r3v3rs3/auth/logout"))
            .header(COOKIE, session.clone())
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert!(resp.headers()[SET_COOKIE].to_str()?.contains("Max-Age=0"));

        let resp = client
            .get(port.http_url("/private?page=1"))
            .header(COOKIE, session)
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::FOUND);

        Ok(())
    })
    .await?;

    mock_private.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn only_the_accounts_that_see_the_proxy_keep_a_session() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("GET", "/")
        .with_body("private")
        .create_async()
        .await;

    let proxy = |id: &str| -> anyhow::Result<Option<BTreeSet<ShortId>>> {
        Ok(Some(BTreeSet::from([id.parse()?])))
    };
    let storage = TestStorage::builder()
        .account("admin", "admin-secret", Role::Admin, None)
        .account("outsider", "outsider-secret", Role::Viewer, proxy("other")?)
        .account("member", "member-secret", Role::Viewer, proxy("proxy1")?)
        .ports(vec![http_port_entry("session", &port)])
        .proxies(vec![http_proxy_entry(
            "proxy1",
            "session",
            HttpProxy {
                routes: vec![http_route("/", &upstream.url(), None)],
                upgrade_insecure: false,
                auth: AuthPolicy::Session,
                ..Default::default()
            },
        )])
        .build();

    with_server(storage.clone(), |mut channels| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let login_url = port.http_url("/.r3v3rs3/auth/login");
        let sign_in = |username: &'static str, password: &'static str| {
            client
                .post(login_url.clone())
                .form(&[("username", username), ("password", password)])
                .send()
        };

        // The password is correct, but the account does not see the proxy.
        let resp = sign_in("outsider", "outsider-secret").await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().get(SET_COOKIE).is_none());

        let resp = sign_in("member", "member-secret").await?;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let set_cookie = resp.headers()[SET_COOKIE].to_str()?;
        let session = set_cookie.split(';').next().unwrap_or_default().to_string();
        let private = || {
            client
                .get(port.http_url("/"))
                .header(COOKIE, session.clone())
                .send()
        };
        assert_eq!(private().await?.status(), StatusCode::OK);

        // The session ends when the proxy leaves the proxy list of the account.
        use r3v3rs3::server::rpc::accounts::UpdateAccount;
        use r3v3rs3_api::auth::AccountUpdate;
        call(
            &mut channels,
            UpdateAccount {
                actor: "system".to_string(),
                username: "member".to_string(),
                update: AccountUpdate {
                    role: Role::Viewer,
                    proxies: proxy("other")?,
                    password: None,
                },
            },
        )
        .await??;
        tokio::time::timeout(Duration::from_secs(5), async {
            while private().await?.status() != StatusCode::FOUND {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            anyhow::Ok(())
        })
        .await
        .map_err(|_| anyhow::anyhow!("the session is still valid"))??;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn session_cookie_scoped_to_base_path_and_stripped_on_unprotected_route() -> anyhow::Result<()>
{
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;

    let mock_public = upstream
        .mock("GET", "/hello")
        .match_header("cookie", "custom=val")
        .with_body("public hello ok")
        .expect(1)
        .create_async()
        .await;

    let mut admin_route = http_route("/admin", &upstream.url(), None);
    admin_route.auth = Some(AuthPolicy::Session);

    let public_route = http_route("/public", &upstream.url(), None);

    let storage = TestStorage::builder()
        .accounts(HashMap::from([("admin".to_string(), "secret".to_string())]))
        .ports(vec![http_port_entry("sesscp", &port)])
        .proxies(vec![http_proxy_entry(
            "pxsess",
            "sesscp",
            HttpProxy {
                routes: vec![admin_route, public_route],
                ..Default::default()
            },
        )])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;

        let login_url = port.http_url("/admin/.r3v3rs3/auth/login");
        let resp = client
            .post(login_url)
            .form(&[
                ("username", "admin"),
                ("password", "secret"),
                ("redirect", "/admin/dashboard"),
            ])
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let set_cookie = resp.headers()[SET_COOKIE].to_str()?;
        assert!(
            set_cookie.contains("Path=/admin"),
            "expected Path=/admin, got: {set_cookie}"
        );

        let session = set_cookie.split(';').next().unwrap_or_default();

        let resp = client
            .get(port.http_url("/public/hello"))
            .header(COOKIE, format!("{session}; custom=val"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "public hello ok");
        mock_public.assert_async().await;

        Ok(())
    })
    .await
}

#[tokio::test]
async fn app_cookies_preserved_on_proxied_requests() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut upstream = mockito::Server::new_async().await;

    let mock_upstream = upstream
        .mock("GET", "/data")
        .match_header("cookie", "token=user-jwt-token; user_pref=1")
        .with_body("ok")
        .expect(1)
        .create_async()
        .await;

    let route = http_route("/api", &upstream.url(), None);
    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("padm", &port)])
        .proxies(vec![http_proxy_entry(
            "pxadm",
            "padm",
            HttpProxy {
                routes: vec![route],
                ..Default::default()
            },
        )])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::new();
        let resp = client
            .get(port.http_url("/api/data"))
            .header(
                COOKIE,
                "r3v3rs3_session=internal-session; token=user-jwt-token; user_pref=1",
            )
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "ok");
        mock_upstream.assert_async().await;
        Ok(())
    })
    .await
}
