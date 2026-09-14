use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use r3v3rs3::{
    command::ServerCommand,
    server::rpc::{proxies::GetProxyStatus, ErasedRpcMethod, RpcWrapper},
};
use r3v3rs3_api::{
    cache::CacheConfig,
    multiaddr::Multiaddr,
    port::UpstreamServer,
    proxy::{HttpProxy, ProxyKind, ProxyStatus, Route, Server, TcpProxy, UdpProxy},
    upstream::{CircuitBreaker, HealthCheck, LoadBalancing, UpstreamTimeouts},
};
use std::{
    collections::HashMap,
    future::{Future, IntoFuture},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};
use url::Url;

mod common;
use common::{alloc_tcp_port, alloc_udp_port, http_route, port_entry, proxy_entry, with_server};
use common::{serve_http_upstream, TestPort, TestStorage};

/// Starts a listener that accepts TCP connections and never sends a byte, so a TLS handshake
/// with it does not finish.
async fn start_silent_upstream() -> anyhow::Result<TestPort> {
    let port = alloc_tcp_port().await?;
    let listener = TcpListener::bind(port.socket_addr()).await?;
    tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            open.push(stream);
        }
    });
    Ok(port)
}

/// Starts an HTTP upstream server that answers every request with its name. The response can be
/// cached for a minute.
async fn start_named_upstream(name: &'static str) -> anyhow::Result<Url> {
    serve_http_upstream(
        Router::new().fallback(move || async move { ([("cache-control", "max-age=60")], name) }),
    )
    .await
}

/// Starts an HTTP upstream server that answers `/health` with 500 and every other request with
/// its name.
async fn start_failing_health_upstream(name: &'static str) -> anyhow::Result<Url> {
    let unhealthy = get(|| async { StatusCode::INTERNAL_SERVER_ERROR });
    serve_http_upstream(
        Router::new()
            .route("/health", unhealthy)
            .fallback(move || async move { name }),
    )
    .await
}

/// Starts a TCP upstream server that sends its name and closes each connection.
async fn start_named_tcp_upstream(name: &'static str) -> anyhow::Result<TestPort> {
    let port = alloc_tcp_port().await?;
    let listener = TcpListener::bind(port.socket_addr()).await?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _ = stream.write_all(name.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Ok(port)
}

/// Starts a UDP upstream server that answers each packet with its name.
async fn start_named_udp_upstream(name: &'static str) -> anyhow::Result<TestPort> {
    let port = alloc_udp_port().await?;
    let socket = UdpSocket::bind(port.socket_addr()).await?;
    tokio::spawn(async move {
        let mut buf = [0; 64];
        while let Ok((_, addr)) = socket.recv_from(&mut buf).await {
            let _ = socket.send_to(name.as_bytes(), addr).await;
        }
    });
    Ok(port)
}

fn timeouts(connect: Duration, request: Duration) -> UpstreamTimeouts {
    UpstreamTimeouts { connect, request }
}

fn no_passive_check() -> HealthCheck {
    HealthCheck {
        max_fails: 0,
        ..Default::default()
    }
}

/// Builds a storage with one port and one proxy for each `(id, listen, kind)`. The id of the
/// port is the id of the proxy with a `p` suffix.
fn storage(proxies: Vec<(&str, Multiaddr, ProxyKind)>) -> TestStorage {
    let ports = proxies
        .iter()
        .map(|(id, listen, _)| port_entry(&format!("{id}p"), listen.clone()))
        .collect();
    let proxies = proxies
        .into_iter()
        .map(|(id, _, kind)| proxy_entry(id, &format!("{id}p"), kind))
        .collect();
    TestStorage::builder().ports(ports).proxies(proxies).build()
}

fn http(proxy: HttpProxy) -> ProxyKind {
    ProxyKind::Http(Box::new(proxy))
}

/// An HTTP proxy for `localhost` with one route to the servers.
fn balanced_http(
    urls: &[Url],
    load_balancing: LoadBalancing,
    health_check: HealthCheck,
) -> ProxyKind {
    let servers = urls
        .iter()
        .map(|url| Server::new(url.as_str().parse().unwrap()))
        .collect();
    http(HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![Route {
            servers,
            ..http_route("/", urls[0].as_str(), None)
        }],
        load_balancing,
        health_check,
        ..Default::default()
    })
}

fn tcp_servers(ports: &[&TestPort]) -> Vec<UpstreamServer> {
    ports
        .iter()
        .map(|port| UpstreamServer::new(port.multiaddr_tcp()))
        .collect()
}

async fn get_body(url: Url) -> anyhow::Result<String> {
    Ok(reqwest::get(url).await?.error_for_status()?.text().await?)
}

async fn read_name(addr: SocketAddr) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    let mut name = String::new();
    timeout(Duration::from_secs(5), stream.read_to_string(&mut name)).await??;
    Ok(name)
}

/// Reads a server name `times` times, and counts the answers of each server.
async fn count_names<F, Fut>(times: usize, read: F) -> anyhow::Result<HashMap<String, usize>>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let mut counts = HashMap::new();
    for _ in 0..times {
        *counts.entry(read().await?).or_default() += 1;
    }
    Ok(counts)
}

fn counts(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
    pairs
        .iter()
        .map(|(name, count)| (name.to_string(), *count))
        .collect()
}

#[tokio::test]
async fn http_request_timeout_returns_gateway_timeout() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let app = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            "slow"
        }),
    );
    let listener = TcpListener::bind(upstream_port.socket_addr()).await?;
    tokio::spawn(axum::serve(listener, app).into_future());

    let upstream = upstream_port.http_url("/");
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![
            Route {
                timeouts: Some(timeouts(Duration::from_secs(10), Duration::from_secs(5))),
                ..http_route("/patient", upstream.as_str(), None)
            },
            http_route("/", upstream.as_str(), None),
        ],
        timeouts: timeouts(Duration::from_secs(10), Duration::from_millis(200)),
        ..Default::default()
    };
    let config = storage(vec![("reqtime", proxy_port.multiaddr_http(), http(proxy))]);

    with_server(config, |_| async move {
        let started = Instant::now();
        let resp = reqwest::get(proxy_port.http_url("/slow")).await?;
        assert_eq!(resp.status(), 504);
        assert!(started.elapsed() < Duration::from_secs(1));

        let resp = reqwest::get(proxy_port.http_url("/patient/slow")).await?;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await?, "slow");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http_connect_timeout_covers_the_tls_handshake() -> anyhow::Result<()> {
    let upstream_port = start_silent_upstream().await?;
    let proxy_port = alloc_tcp_port().await?;

    let upstream = format!("https://localhost:{}/", upstream_port.socket_addr().port());
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        // The request timeout is disabled, so only the connect timeout can end the request.
        timeouts: timeouts(Duration::from_millis(300), Duration::ZERO),
        ..Default::default()
    };
    let config = storage(vec![("connto", proxy_port.multiaddr_http(), http(proxy))]);

    with_server(config, |_| async move {
        let request = reqwest::get(proxy_port.http_url("/"));
        let resp = timeout(Duration::from_secs(5), request).await??;
        assert_eq!(resp.status(), 504);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn tcp_connect_timeout_closes_the_client_connection() -> anyhow::Result<()> {
    let upstream_port = start_silent_upstream().await?;
    let proxy_port = alloc_tcp_port().await?;

    let tcp = TcpProxy {
        upstream_servers: vec![UpstreamServer::new(
            format!(
                "/dns/localhost/tcp/{}/tls",
                upstream_port.socket_addr().port()
            )
            .parse()
            .unwrap(),
        )],
        connect_timeout: Duration::from_millis(300),
        ..Default::default()
    };
    let config = storage(vec![(
        "tcptime",
        proxy_port.multiaddr_tcp(),
        ProxyKind::Tcp(tcp),
    )]);

    with_server(config, |_| async move {
        let mut client = TcpStream::connect(proxy_port.socket_addr()).await?;
        let mut buf = [0; 16];
        let read = timeout(Duration::from_secs(5), client.read(&mut buf)).await?;
        assert_eq!(read.unwrap_or(0), 0);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http_load_balancing_policies() -> anyhow::Result<()> {
    let urls = [
        start_named_upstream("a").await?,
        start_named_upstream("b").await?,
    ];
    let round_robin_port = alloc_tcp_port().await?;
    let first_port = alloc_tcp_port().await?;
    let config = storage(vec![
        (
            "httprr",
            round_robin_port.multiaddr_http(),
            balanced_http(&urls, LoadBalancing::RoundRobin, HealthCheck::default()),
        ),
        (
            "httpfst",
            first_port.multiaddr_http(),
            balanced_http(&urls, LoadBalancing::First, HealthCheck::default()),
        ),
    ]);

    with_server(config, |_| async move {
        let round_robin = count_names(4, || get_body(round_robin_port.http_url("/"))).await?;
        assert_eq!(round_robin, counts(&[("a", 2), ("b", 2)]));

        let first = count_names(3, || get_body(first_port.http_url("/"))).await?;
        assert_eq!(first, counts(&[("a", 3)]));
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http_failover_and_passive_health_check() -> anyhow::Result<()> {
    let urls = [
        alloc_tcp_port().await?.http_url("/"),
        start_named_upstream("live").await?,
    ];
    let retry_port = alloc_tcp_port().await?;
    let passive_port = alloc_tcp_port().await?;
    let config = storage(vec![
        (
            "retry",
            retry_port.multiaddr_http(),
            balanced_http(&urls, LoadBalancing::First, no_passive_check()),
        ),
        (
            "passive",
            passive_port.multiaddr_http(),
            balanced_http(&urls, LoadBalancing::First, HealthCheck::default()),
        ),
    ]);

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        // The first server refuses the connection, so a request without a body goes to the next
        // server. A request with a body is not sent again.
        let retried = count_names(2, || get_body(retry_port.http_url("/"))).await?;
        assert_eq!(retried, counts(&[("live", 2)]));
        let resp = client
            .post(retry_port.http_url("/"))
            .body("data")
            .send()
            .await?;
        assert_eq!(resp.status(), 502);

        // After one failure, the passive health check moves the first server behind the second
        // server, so the next request with a body reaches the second server.
        assert_eq!(get_body(passive_port.http_url("/")).await?, "live");
        let resp = client
            .post(passive_port.http_url("/"))
            .body("data")
            .send()
            .await?;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await?, "live");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn active_health_check_marks_a_failing_server_unhealthy() -> anyhow::Result<()> {
    let urls = [
        start_failing_health_upstream("sick").await?,
        start_named_upstream("live").await?,
    ];
    let proxy_port = alloc_tcp_port().await?;
    // The passive check is disabled, so only the active check can move the first server.
    let health_check = HealthCheck {
        max_fails: 0,
        interval: Duration::from_millis(200),
        timeout: Duration::from_secs(1),
        path: "/health".into(),
        ..Default::default()
    };
    let config = storage(vec![(
        "active",
        proxy_port.multiaddr_http(),
        balanced_http(&urls, LoadBalancing::First, health_check),
    )]);

    with_server(config, |mut channels| async move {
        let mut body = String::new();
        for _ in 0..50 {
            body = get_body(proxy_port.http_url("/")).await?;
            if body == "live" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(body, "live");

        let id = "active".parse().unwrap();
        let arg = Box::new(RpcWrapper::new(GetProxyStatus { id })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod { id: 1, arg })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        let status = callback
            .result?
            .downcast::<ProxyStatus>()
            .map_err(|_| anyhow::anyhow!("the RPC returned another type"))?;
        let health = status
            .upstreams
            .iter()
            .map(|server| {
                (
                    server.addr.as_str(),
                    server.healthy,
                    server.last_error.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            health,
            vec![
                (
                    urls[0].as_str(),
                    false,
                    Some("the active check received status 500 Internal Server Error"),
                ),
                (urls[1].as_str(), true, None),
            ]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn cache_key_does_not_depend_on_the_upstream_server() -> anyhow::Result<()> {
    let urls = [
        start_named_upstream("a").await?,
        start_named_upstream("b").await?,
    ];
    let proxy_port = alloc_tcp_port().await?;
    let ProxyKind::Http(mut proxy) =
        balanced_http(&urls, LoadBalancing::RoundRobin, HealthCheck::default())
    else {
        unreachable!();
    };
    proxy.cache = CacheConfig {
        enabled: true,
        ..Default::default()
    };
    let config = storage(vec![(
        "cachelb",
        proxy_port.multiaddr_http(),
        ProxyKind::Http(proxy),
    )]);

    with_server(config, |_| async move {
        let x_cache = |resp: &reqwest::Response| resp.headers().get("x-cache").cloned();
        let first = reqwest::get(proxy_port.http_url("/page")).await?;
        assert_eq!(
            x_cache(&first).as_ref().map(|v| v.as_bytes()),
            Some(&b"MISS"[..])
        );
        let body = first.text().await?;

        let second = reqwest::get(proxy_port.http_url("/page")).await?;
        assert_eq!(
            x_cache(&second).as_ref().map(|v| v.as_bytes()),
            Some(&b"HIT"[..])
        );
        assert_eq!(second.text().await?, body);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn tcp_load_balancing_and_failover() -> anyhow::Result<()> {
    let a = start_named_tcp_upstream("a").await?;
    let b = start_named_tcp_upstream("b").await?;
    let closed = alloc_tcp_port().await?;
    let round_robin_port = alloc_tcp_port().await?;
    let failover_port = alloc_tcp_port().await?;
    let round_robin = TcpProxy {
        upstream_servers: tcp_servers(&[&a, &b]),
        ..Default::default()
    };
    let failover = TcpProxy {
        upstream_servers: tcp_servers(&[&closed, &a]),
        load_balancing: LoadBalancing::First,
        health_check: no_passive_check(),
        ..Default::default()
    };
    let config = storage(vec![
        (
            "tcprr",
            round_robin_port.multiaddr_tcp(),
            ProxyKind::Tcp(round_robin),
        ),
        (
            "tcpfo",
            failover_port.multiaddr_tcp(),
            ProxyKind::Tcp(failover),
        ),
    ]);

    with_server(config, |_| async move {
        let round_robin = count_names(4, || read_name(round_robin_port.socket_addr())).await?;
        assert_eq!(round_robin, counts(&[("a", 2), ("b", 2)]));

        let failover = count_names(2, || read_name(failover_port.socket_addr())).await?;
        assert_eq!(failover, counts(&[("a", 2)]));
        Ok(())
    })
    .await
}

#[tokio::test]
async fn udp_sessions_use_the_servers_in_turn() -> anyhow::Result<()> {
    let servers = [
        start_named_udp_upstream("a").await?,
        start_named_udp_upstream("b").await?,
    ];
    let proxy_port = alloc_udp_port().await?;
    let udp = UdpProxy {
        upstream_servers: servers
            .iter()
            .map(|port| UpstreamServer::new(port.multiaddr_udp()))
            .collect(),
        ..Default::default()
    };
    let config = storage(vec![(
        "udprr",
        proxy_port.multiaddr_udp(),
        ProxyKind::Udp(udp),
    )]);

    with_server(config, |_| async move {
        let proxy = proxy_port.socket_addr();
        let mut names = Vec::new();
        for _ in 0..2 {
            let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
            // A session keeps its server, so both packets of a client reach the same server.
            for _ in 0..2 {
                client.send_to(b"ping", proxy).await?;
                let mut buf = [0; 64];
                let (size, _) =
                    timeout(Duration::from_secs(5), client.recv_from(&mut buf)).await??;
                names.push(String::from_utf8_lossy(&buf[..size]).to_string());
            }
        }
        assert_eq!(names[0], names[1]);
        assert_eq!(names[2], names[3]);
        assert_ne!(names[0], names[2]);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http_weights_split_requests() -> anyhow::Result<()> {
    let a = start_named_upstream("a").await?;
    let b = start_named_upstream("b").await?;
    let proxy_port = alloc_tcp_port().await?;
    let servers = vec![
        Server {
            weight: 3,
            ..Server::new(a.as_str().parse()?)
        },
        Server::new(b.as_str().parse()?),
    ];
    let proxy = http(HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![Route {
            servers,
            ..Default::default()
        }],
        ..Default::default()
    });
    let config = storage(vec![("httpwt", proxy_port.multiaddr_http(), proxy)]);

    with_server(config, |_| async move {
        let names = count_names(8, || get_body(proxy_port.http_url("/"))).await?;
        assert_eq!(names, counts(&[("a", 6), ("b", 2)]));
        Ok(())
    })
    .await
}

#[tokio::test]
async fn tcp_and_udp_servers_with_weight_zero_get_no_traffic() -> anyhow::Result<()> {
    let tcp_a = start_named_tcp_upstream("a").await?;
    let tcp_b = start_named_tcp_upstream("b").await?;
    let udp_a = start_named_udp_upstream("a").await?;
    let udp_b = start_named_udp_upstream("b").await?;
    let tcp_port = alloc_tcp_port().await?;
    let udp_port = alloc_udp_port().await?;
    let mut drained_tcp = tcp_servers(&[&tcp_a, &tcp_b]);
    drained_tcp[0].weight = 0;
    let tcp = TcpProxy {
        upstream_servers: drained_tcp,
        ..Default::default()
    };
    let udp = UdpProxy {
        upstream_servers: vec![
            UpstreamServer {
                weight: 0,
                ..UpstreamServer::new(udp_a.multiaddr_udp())
            },
            UpstreamServer::new(udp_b.multiaddr_udp()),
        ],
        ..Default::default()
    };
    let config = storage(vec![
        ("tcpdrn", tcp_port.multiaddr_tcp(), ProxyKind::Tcp(tcp)),
        ("udpdrn", udp_port.multiaddr_udp(), ProxyKind::Udp(udp)),
    ]);

    with_server(config, |_| async move {
        let names = count_names(4, || read_name(tcp_port.socket_addr())).await?;
        assert_eq!(names, counts(&[("b", 4)]));

        let proxy = udp_port.socket_addr();
        for _ in 0..3 {
            let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
            client.send_to(b"ping", proxy).await?;
            let mut buf = [0; 64];
            let (size, _) = timeout(Duration::from_secs(5), client.recv_from(&mut buf)).await??;
            assert_eq!(&buf[..size], b"b");
        }
        Ok(())
    })
    .await
}

/// Starts an HTTP upstream server that answers 503 while `failing` is true, and its name
/// otherwise.
async fn start_switchable_upstream(
    name: &'static str,
    failing: Arc<AtomicBool>,
) -> anyhow::Result<Url> {
    serve_http_upstream(Router::new().fallback(move || {
        let failing = failing.clone();
        async move {
            if failing.load(Ordering::SeqCst) {
                (StatusCode::SERVICE_UNAVAILABLE, "down").into_response()
            } else {
                name.into_response()
            }
        }
    }))
    .await
}

#[tokio::test]
async fn http_circuit_breaker_opens_and_recovers() -> anyhow::Result<()> {
    let failing = Arc::new(AtomicBool::new(true));
    let a = start_switchable_upstream("a", failing.clone()).await?;
    let b = start_named_upstream("b").await?;
    let proxy_port = alloc_tcp_port().await?;
    let proxy = http(HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![
            Route {
                servers: vec![
                    Server::new(a.as_str().parse()?),
                    Server::new(b.as_str().parse()?),
                ],
                ..Default::default()
            },
            Route {
                path: "/solo".into(),
                servers: vec![Server::new(a.as_str().parse()?)],
                ..Default::default()
            },
        ],
        load_balancing: LoadBalancing::First,
        health_check: no_passive_check(),
        circuit_breaker: CircuitBreaker {
            enabled: true,
            failure_ratio: 50,
            min_requests: 2,
            window: Duration::from_secs(10),
            open_duration: Duration::from_secs(1),
        },
        ..Default::default()
    });
    let config = storage(vec![("breaker", proxy_port.multiaddr_http(), proxy)]);

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let get = |path: &str| {
            let request = client.get(proxy_port.http_url(path));
            async move {
                let res = request.send().await?;
                anyhow::Ok((res.status().as_u16(), res.text().await?))
            }
        };

        // Each route has its own circuit. Two 503 answers of a open both circuits.
        for path in ["/", "/", "/solo", "/solo"] {
            assert_eq!(get(path).await?, (503, "down".to_string()), "{path}");
        }
        assert_eq!(get("/").await?, (200, "b".to_string()));
        let (status, body) = get("/solo").await?;
        assert_eq!(status, 503);
        assert_ne!(body, "down", "the proxy answers without contacting a");

        // After the open duration, a trial request reaches a and closes the circuit.
        failing.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(get("/").await?, (200, "a".to_string()));
        assert_eq!(get("/").await?, (200, "a".to_string()));
        Ok(())
    })
    .await
}
