use axum::http::Uri;
use axum::Router;
use r3v3rs3_api::cidr::parse_cidr_list;
use r3v3rs3_api::fixed_response::{FixedRedirect, FixedResponse, FixedStatus};
use r3v3rs3_api::policy::IpFilter;
use r3v3rs3_api::proxy::{HttpProxy, Route};
use r3v3rs3_api::redirect::{RedirectRule, RedirectStatus};
use r3v3rs3_api::rewrite::PathRewrite;
use reqwest::redirect::Policy;
use url::Url;

mod common;
use common::{alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server};
use common::{serve_http_upstream, TestPort, TestStorage};

/// Starts an HTTP upstream server that answers every request with its name.
async fn start_named_upstream(name: &'static str) -> anyhow::Result<Url> {
    serve_http_upstream(Router::new().fallback(move || async move { name })).await
}

fn proxy(vhosts: &[&str], routes: Vec<Route>) -> HttpProxy {
    HttpProxy {
        vhosts: vhosts.iter().map(|vhost| vhost.parse().unwrap()).collect(),
        routes,
        ..Default::default()
    }
}

async fn get_name(port: &TestPort, host: &str, path: &str) -> anyhow::Result<String> {
    Ok(reqwest::Client::new()
        .get(port.http_url(path))
        .header("host", host)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

#[tokio::test]
async fn the_most_specific_host_and_the_longest_path_win() -> anyhow::Result<()> {
    let default = start_named_upstream("default").await?;
    let api = start_named_upstream("api").await?;
    let app = start_named_upstream("app").await?;
    let wildcard = start_named_upstream("wildcard").await?;
    let port = alloc_tcp_port().await?;

    // Every less specific route and proxy comes first, so the first match would be wrong.
    let any_host = proxy(
        &[],
        vec![
            http_route("/", default.as_str(), None),
            http_route("/api", api.as_str(), None),
        ],
    );
    let wildcard_host = proxy(
        &["*.example.test"],
        vec![http_route("/", wildcard.as_str(), None)],
    );
    let exact_host = proxy(
        &["app.example.test"],
        vec![http_route("/", app.as_str(), None)],
    );
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("route", &port)])
        .proxies(vec![
            http_proxy_entry("any", "route", any_host),
            http_proxy_entry("wild", "route", wildcard_host),
            http_proxy_entry("exact", "route", exact_host),
        ])
        .build();

    with_server(config, |_| async move {
        assert_eq!(get_name(&port, "other.test", "/api/users").await?, "api");
        assert_eq!(get_name(&port, "other.test", "/about").await?, "default");
        assert_eq!(get_name(&port, "x.example.test", "/api").await?, "wildcard");
        assert_eq!(get_name(&port, "app.example.test", "/api").await?, "app");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn fixed_responses_answer_without_an_upstream_server() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let fixed = |path: &str, response: FixedResponse| Route {
        path: path.into(),
        response: Some(response),
        ..Default::default()
    };
    let redirect = |target: &str, status: RedirectStatus, preserve_path: bool| {
        FixedResponse::Redirect(FixedRedirect {
            target: target.into(),
            status,
            preserve_path,
        })
    };
    let not_found = FixedResponse::Status(FixedStatus {
        status: 404,
        body: "not here".into(),
    });
    let deny_all = IpFilter {
        allow: vec![],
        deny: parse_cidr_list("0.0.0.0/0, ::/0")?,
    };
    let routes = vec![
        fixed(
            "/old",
            redirect("https://new.test/", RedirectStatus::MovedPermanently, true),
        ),
        fixed(
            "/landing",
            redirect("https://new.test/landing", RedirectStatus::Found, false),
        ),
        fixed("/gone", not_found.clone()),
        Route {
            ip_filter: Some(deny_all),
            ..fixed("/private", not_found)
        },
    ];
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("fixed", &port)])
        .proxies(vec![http_proxy_entry("fixed", "fixed", proxy(&[], routes))])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let get = |path: &str| client.get(port.http_url(path)).send();
        let moved = get("/old/a?b=1").await?;
        assert_eq!(moved.status(), 301);
        assert_eq!(moved.headers()["location"], "https://new.test/old/a?b=1");
        let landing = get("/landing/x").await?;
        assert_eq!(landing.status(), 302);
        assert_eq!(landing.headers()["location"], "https://new.test/landing");
        let gone = get("/gone").await?;
        assert_eq!(gone.status(), 404);
        assert_eq!(gone.headers()["content-type"], "text/plain; charset=utf-8");
        assert_eq!(gone.text().await?, "not here");
        // The IP filter runs before the fixed response.
        assert_eq!(get("/private").await?.status(), 403);
        Ok(())
    })
    .await
}

/// Starts an HTTP upstream server that answers with the path and the query that it received.
async fn start_path_echo() -> anyhow::Result<Url> {
    serve_http_upstream(Router::new().fallback(|uri: Uri| async move {
        uri.path_and_query()
            .map(|path| path.to_string())
            .unwrap_or_default()
    }))
    .await
}

#[tokio::test]
async fn the_server_path_and_the_request_path_are_joined_once() -> anyhow::Result<()> {
    let echo = start_path_echo().await?;
    let port = alloc_tcp_port().await?;
    let routes = vec![
        http_route("/slash", echo.join("base/")?.as_str(), None),
        http_route("/plain", echo.join("base")?.as_str(), None),
        http_route("/root", echo.as_str(), None),
    ];
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("join", &port)])
        .proxies(vec![http_proxy_entry("join", "join", proxy(&[], routes))])
        .build();

    with_server(config, |_| async move {
        let cases = [
            ("/slash/x?q=1", "/base/x?q=1"),
            ("/slash/x/", "/base/x/"),
            ("/slash/a%20b", "/base/a%20b"),
            ("/slash", "/base/"),
            ("/plain/x", "/base/x"),
            ("/root/x", "/x"),
        ];
        for (request, expected) in cases {
            assert_eq!(
                get_name(&port, "join.test", request).await?,
                expected,
                "{request}"
            );
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn route_paths_are_rewritten_before_the_server_path() -> anyhow::Result<()> {
    let server = start_path_echo().await?.join("base/")?;
    let port = alloc_tcp_port().await?;
    let route = |path: &str, rewrite: PathRewrite| Route {
        rewrite,
        ..http_route(path, server.as_str(), None)
    };
    let routes = vec![
        route(
            "/keep",
            PathRewrite {
                strip_prefix: false,
                ..Default::default()
            },
        ),
        route(
            "/v1",
            PathRewrite {
                add_prefix: "/v2".into(),
                ..Default::default()
            },
        ),
        route(
            "/re",
            PathRewrite {
                regex: Some("^/items/([0-9]+)$".parse()?),
                replacement: "/item/${1}".into(),
                add_prefix: "/p".into(),
                ..Default::default()
            },
        ),
    ];
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("rewrite", &port)])
        .proxies(vec![http_proxy_entry(
            "rewrite",
            "rewrite",
            proxy(&[], routes),
        )])
        .build();

    with_server(config, |_| async move {
        let cases = [
            ("/keep/a?q=1", "/base/keep/a?q=1"),
            ("/keep", "/base/keep"),
            ("/v1/users", "/base/v2/users"),
            ("/v1", "/base/v2"),
            ("/re/items/42?full=1", "/base/p/item/42?full=1"),
            ("/re/other", "/base/p/other"),
        ];
        for (request, expected) in cases {
            assert_eq!(
                get_name(&port, "rewrite.test", request).await?,
                expected,
                "{request}"
            );
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn redirect_rules_answer_after_the_ip_filter() -> anyhow::Result<()> {
    let app = start_named_upstream("app").await?;
    let port = alloc_tcp_port().await?;
    let redirects = [
        r"301 ^open\.test/old/(.*)$ https://new.test/${1}",
        r"308 ^open\.test/old/.*$ https://unused.test/",
        r"307 ^open\.test/temp$ /elsewhere",
    ]
    .iter()
    .map(|line| RedirectRule::parse_line(line))
    .collect::<Result<Vec<_>, _>>()?;
    let open = HttpProxy {
        redirects: redirects.clone(),
        ..proxy(&["open.test"], vec![http_route("/", app.as_str(), None)])
    };
    let local = IpFilter {
        deny: parse_cidr_list("127.0.0.0/8, ::1")?,
        ..Default::default()
    };
    let closed = HttpProxy {
        redirects,
        ..proxy(
            &["closed.test"],
            vec![http_route("/", app.as_str(), Some(local))],
        )
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("redirect", &port)])
        .proxies(vec![
            http_proxy_entry("open", "redirect", open),
            http_proxy_entry("closed", "redirect", closed),
        ])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let get = |host: &'static str, path: &str| {
            let request = client.get(port.http_url(path)).header("host", host);
            async move {
                let res = request.send().await?;
                let location = res
                    .headers()
                    .get("location")
                    .map(|value| value.to_str().map(str::to_string))
                    .transpose()?;
                anyhow::Ok((res.status().as_u16(), location))
            }
        };
        let moved = Some("https://new.test/a/b?x=1".to_string());
        assert_eq!(get("open.test", "/old/a/b?x=1").await?, (301, moved));
        let temporary = Some("/elsewhere".to_string());
        assert_eq!(get("open.test", "/temp").await?, (307, temporary));
        assert_eq!(get("open.test", "/other").await?, (200, None));
        assert_eq!(get("closed.test", "/old/a").await?, (403, None));
        Ok(())
    })
    .await
}
