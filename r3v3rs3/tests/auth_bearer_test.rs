use mockito::Matcher;
use r3v3rs3_api::{
    policy::{AuthPolicy, BearerAuth, BearerToken},
    proxy::HttpProxy,
};
use reqwest::{StatusCode, header::WWW_AUTHENTICATE};
use sha2::{Digest, Sha256};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

const TOKEN: &str = "token-for-ci-0123456789";

#[tokio::test]
async fn bearer_auth_protects_routes() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let mock_api = server
        .mock("GET", "/api")
        .match_header("authorization", Matcher::Missing)
        .with_body("api")
        .expect(1)
        .create_async()
        .await;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        upgrade_insecure: false,
        auth: AuthPolicy::Bearer(BearerAuth {
            tokens: vec![BearerToken {
                name: "ci".into(),
                token: String::new(),
                token_hash: hex::encode(Sha256::digest(TOKEN)),
                token_set: false,
            }],
        }),
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("bearer", &port)])
        .proxies(vec![http_proxy_entry("proxy1", "bearer", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/api");

        let resp = client.get(url.clone()).send().await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(WWW_AUTHENTICATE).unwrap(),
            "Bearer realm=\"r3v3rs3\""
        );

        let resp = client
            .get(url.clone())
            .bearer_auth("token-for-ci-wrong")
            .send()
            .await?;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(WWW_AUTHENTICATE).unwrap(),
            "Bearer realm=\"r3v3rs3\", error=\"invalid_token\""
        );

        let resp = client.get(url).bearer_auth(TOKEN).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await?, "api");

        Ok(())
    })
    .await?;

    mock_api.assert_async().await;
    Ok(())
}
