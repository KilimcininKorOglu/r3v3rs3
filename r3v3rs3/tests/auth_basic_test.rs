use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use mockito::Matcher;
use r3v3rs3_api::{
    policy::{AuthPolicy, BasicAuth, BasicAuthUser},
    proxy::HttpProxy,
};
use reqwest::{header::WWW_AUTHENTICATE, StatusCode};

mod common;
use common::{
    alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server, TestStorage,
};

fn basic_auth(realm: &str, username: &str, password: &str) -> AuthPolicy {
    let salt = SaltString::generate(rand::thread_rng());
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string();
    AuthPolicy::Basic(BasicAuth {
        realm: realm.into(),
        users: vec![BasicAuthUser {
            username: username.into(),
            password: String::new(),
            password_hash,
        }],
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
        let client = reqwest::Client::new();
        let url = port.http_url("/private");

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
            .get(port.http_url("/public"))
            .bearer_auth("upstream-token")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "public");

        Ok(())
    })
    .await?;

    mock_private.assert_async().await;
    mock_public.assert_async().await;
    Ok(())
}
