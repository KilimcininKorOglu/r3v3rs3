use ipnet::IpNet;
use r3v3rs3_api::{
    app::{AppConfig, GeoIpConfig},
    client_ip::ClientIpConfig,
    policy::IpFilter,
    proxy::HttpProxy,
};
use reqwest::StatusCode;
use std::path::PathBuf;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn nets(values: &[&str]) -> Vec<IpNet> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

fn loopback() -> Vec<IpNet> {
    nets(&["127.0.0.0/8", "::1/128"])
}

#[tokio::test]
async fn test_geoip_country_filter() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let _mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_body("hello country")
        .create_async()
        .await;

    let test_mmdb = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("GeoIP2-Country-Test.mmdb");

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        client_ip: ClientIpConfig {
            trust_cdn: true,
            trusted_proxies: loopback(),
        },
        ip_filter: IpFilter {
            allow_countries: vec!["GB".into()],
            ..Default::default()
        },
        ..Default::default()
    };

    let config = TestStorage::builder()
        .config(AppConfig {
            geoip: GeoIpConfig {
                database: Some(test_mmdb),
                ..Default::default()
            },
            ..Default::default()
        })
        .ports(vec![http_port_entry("http", &port)])
        .proxies(vec![http_proxy_entry("test", "http", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let url = port.http_url("/");

        // 81.2.69.160 is GB in the test mmdb -> 200 OK
        let res_gb = client
            .get(url.clone())
            .header("X-Forwarded-For", "81.2.69.160")
            .send()
            .await?;
        assert_eq!(res_gb.status(), StatusCode::OK);

        // Unknown or non-GB country IP -> 403 Forbidden
        let res_other = client
            .get(url)
            .header("X-Forwarded-For", "1.2.3.4")
            .send()
            .await?;
        assert_eq!(res_other.status(), StatusCode::FORBIDDEN);

        Ok(())
    })
    .await
}
