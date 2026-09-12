use mockito::Matcher;
use r3v3rs3_api::{
    policy::{AuthPolicy, ForwardAuth},
    proxy::HttpProxy,
};
use reqwest::{header::LOCATION, redirect::Policy, StatusCode};
use std::time::Duration;

mod common;
use common::{
    alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server, TestStorage,
};

fn proxy(upstream: &str, auth_url: String) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        upgrade_insecure: false,
        auth: AuthPolicy::Forward(Box::new(ForwardAuth {
            url: auth_url.parse().unwrap(),
            response_headers: vec!["X-Auth-User".into()],
            timeout: Duration::from_secs(5),
        })),
        ..Default::default()
    }
}

#[tokio::test]
async fn forward_auth_allows_and_denies_requests() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let broken_port = alloc_tcp_port().await?;
    let closed_port = alloc_tcp_port().await?;
    let mut auth = mockito::Server::new_async().await;
    let mut upstream = mockito::Server::new_async().await;

    let mock_allow = auth
        .mock("GET", "/verify")
        .match_header("cookie", "session=good")
        .match_header("x-forwarded-method", "GET")
        .match_header("x-forwarded-proto", "http")
        .match_header("x-forwarded-host", "localhost")
        .match_header("x-forwarded-uri", "/private?page=1")
        .with_header("x-auth-user", "alice")
        .expect(1)
        .create_async()
        .await;
    let mock_deny = auth
        .mock("GET", "/verify")
        .match_header("cookie", Matcher::Missing)
        .with_status(302)
        .with_header("location", "/login")
        .with_body("login")
        .expect(1)
        .create_async()
        .await;
    let mock_upstream = upstream
        .mock("GET", "/private?page=1")
        .match_header("x-auth-user", "alice")
        .match_header("cookie", "session=good")
        .with_body("private")
        .expect(1)
        .create_async()
        .await;

    let config = TestStorage::builder()
        .ports(vec![
            http_port_entry("fwd", &port),
            http_port_entry("broken", &broken_port),
        ])
        .proxies(vec![
            http_proxy_entry(
                "proxy1",
                "fwd",
                proxy(&upstream.url(), format!("{}/verify", auth.url())),
            ),
            http_proxy_entry(
                "proxy2",
                "broken",
                proxy(
                    &upstream.url(),
                    format!("http://{}/verify", closed_port.socket_addr()),
                ),
            ),
        ])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;

        let resp = client
            .get(port.http_url("/private?page=1"))
            .header("cookie", "session=good")
            .header("x-auth-user", "mallory")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "private");

        let resp = client.get(port.http_url("/private?page=1")).send().await?;
        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(resp.headers().get(LOCATION).unwrap(), "/login");
        assert_eq!(resp.text().await?, "login");

        let resp = client
            .get(broken_port.http_url("/private?page=1"))
            .header("cookie", "session=good")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

        Ok(())
    })
    .await?;

    mock_allow.assert_async().await;
    mock_deny.assert_async().await;
    mock_upstream.assert_async().await;
    Ok(())
}
