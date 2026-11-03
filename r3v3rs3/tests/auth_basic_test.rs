use argon2::{Argon2, PasswordHasher};
use mockito::Matcher;
use r3v3rs3_api::{
    policy::{AuthPolicy, BasicAuth, BasicAuthUser},
    proxy::HttpProxy,
};
use reqwest::{StatusCode, Url, header::WWW_AUTHENTICATE};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn basic_auth(realm: &str, username: &str, password: &str) -> AuthPolicy {
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    AuthPolicy::Basic(BasicAuth {
        realm: realm.into(),
        users: vec![BasicAuthUser {
            username: username.into(),
            password: String::new(),
            password_hash,
            password_set: false,
        }],
        user_file: None,
    })
}

#[tokio::test]
async fn basic_auth_protects_routes() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let mock_private = server
        .mock("GET", "/private")
        .match_header("authorization", Matcher::Missing)
        .with_body("private")
        .expect(2)
        .create_async()
        .await;
    let mock_public = server
        .mock("GET", "/")
        .match_header("authorization", "Bearer upstream-token")
        .with_body("public")
        .expect(1)
        .create_async()
        .await;

    let mut public_route = http_route("/public", &upstream, None);
    public_route.auth = Some(AuthPolicy::None);
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![public_route, http_route("/", &upstream, None)],
        upgrade_insecure: false,
        auth: basic_auth("Staff", "alice", "s3cr:et"),
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("auth", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "auth", proxy)])
        .build();

    with_server(config, |_| async move {
        let url = port.http_url("/private");
        let public_url = port.http_url("/public");
        assert_basic_auth_requests(&url, &public_url).await
    })
    .await?;

    mock_private.assert_async().await;
    mock_public.assert_async().await;
    Ok(())
}

async fn assert_basic_auth_requests(url: &Url, public_url: &Url) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp = client.get(url.clone()).send().await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers().get(WWW_AUTHENTICATE).unwrap(),
        "Basic realm=\"Staff\", charset=\"UTF-8\""
    );

    for (username, password) in [("alice", "wrong"), ("bob", "s3cr:et"), ("alice", "")] {
        let resp = client
            .get(url.clone())
            .basic_auth(username, Some(password))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    for _ in 0..2 {
        let resp = client
            .get(url.clone())
            .basic_auth("alice", Some("s3cr:et"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "private");
    }

    let resp = client
        .get(public_url.clone())
        .bearer_auth("upstream-token")
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await?, "public");
    Ok(())
}

#[tokio::test]
async fn basic_auth_with_user_file_protects_proxy() -> anyhow::Result<()> {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/secret")
        .with_status(200)
        .with_body("secret content")
        .expect_at_least(1)
        .create_async()
        .await;

    let file_name = format!("test-users-{}.txt", std::process::id());
    std::fs::write(&file_name, "developer:supersecret\n")?;

    let port = alloc_tcp_port().await?;
    let proxy = HttpProxy {
        routes: vec![http_route("/", &server.url(), None)],
        auth: AuthPolicy::Basic(BasicAuth {
            realm: "Protected".into(),
            users: vec![],
            user_file: Some(file_name.clone()),
        }),
        ..Default::default()
    };

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("bfiles", "http", proxy)])
        .build();

    with_server(storage, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/secret");

        // 1. Unauthenticated request -> 401
        let resp = client.get(url.clone()).send().await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 2. Wrong password -> 401
        let resp = client
            .get(url.clone())
            .basic_auth("developer", Some("wrongpass"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 3. Valid user from file -> 200
        let resp = client
            .get(url.clone())
            .basic_auth("developer", Some("supersecret"))
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "secret content");

        Ok(())
    })
    .await?;

    mock.assert_async().await;
    let _ = std::fs::remove_file(&file_name);
    Ok(())
}

#[test]
fn basic_auth_user_file_rejects_absolute_and_traversal_paths() {
    let proxy_with_path = |path: &str| HttpProxy {
        routes: vec![http_route("/", "http://127.0.0.1:8080", None)],
        auth: AuthPolicy::Basic(BasicAuth {
            realm: "Protected".into(),
            users: vec![],
            user_file: Some(path.to_string()),
        }),
        ..Default::default()
    };

    assert!(proxy_with_path("users.txt").validate_upstream().is_ok());
    assert!(proxy_with_path("auth/htpasswd").validate_upstream().is_ok());

    assert!(proxy_with_path("/etc/passwd").validate_upstream().is_err());
    assert!(proxy_with_path("../secret.txt").validate_upstream().is_err());
    assert!(proxy_with_path("dir/../../etc/shadow").validate_upstream().is_err());
    assert!(proxy_with_path("~/users.txt").validate_upstream().is_err());
    assert!(proxy_with_path("").validate_upstream().is_err());
}
