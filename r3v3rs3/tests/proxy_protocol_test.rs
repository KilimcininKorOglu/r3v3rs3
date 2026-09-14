use ppp::v2;
use r3v3rs3::certs::Cert;
use r3v3rs3_api::{
    policy::IpFilter,
    port::{PortEntry, UpstreamServer},
    proxy::{HttpProxy, ProxyKind, TcpProxy},
    proxy_protocol::{ProxyProtocolAccept, ProxyProtocolReceive},
    tls::TlsTermination,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{
    rustls::{crypto::ring, pki_types::ServerName, ClientConfig, RootCertStore},
    TlsConnector,
};

mod common;
use common::{
    alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, port_entry, proxy_entry,
    wait_for_listener, with_server, TestStorage,
};

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
const DENIED_SOURCE: &str = "203.0.113.9:51000";
const ALLOWED_SOURCE: &str = "198.51.100.1:51000";

fn receive(trusted: &[&str]) -> ProxyProtocolReceive {
    ProxyProtocolReceive {
        accept: ProxyProtocolAccept::Any,
        trusted: trusted.iter().map(|net| net.parse().unwrap()).collect(),
        timeout: Duration::from_secs(1),
    }
}

fn with_proxy_protocol(mut entry: PortEntry, config: ProxyProtocolReceive) -> PortEntry {
    entry.port.opts.proxy_protocol = Some(config);
    entry
}

/// An HTTP proxy that rejects the clients of 203.0.113.0/24.
fn filtered_proxy(upstream: &str) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        ip_filter: IpFilter {
            allow: vec![],
            deny: vec!["203.0.113.0/24".parse().unwrap()],
        },
        ..Default::default()
    }
}

fn v1_header(source: &str) -> Vec<u8> {
    let source: SocketAddr = source.parse().unwrap();
    format!(
        "PROXY TCP4 {} 127.0.0.1 {} 80\r\n",
        source.ip(),
        source.port()
    )
    .into_bytes()
}

fn v2_header(source: &str) -> Vec<u8> {
    let source: SocketAddr = source.parse().unwrap();
    let destination: SocketAddr = "127.0.0.1:80".parse().unwrap();
    v2::Builder::with_addresses(
        v2::Version::Two | v2::Command::Proxy,
        v2::Protocol::Stream,
        (source, destination),
    )
    .build()
    .unwrap()
}

/// Sends the bytes and returns the status line of the response.
async fn status_line<S>(stream: &mut S, bytes: &[u8]) -> anyhow::Result<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.write_all(bytes).await?;
    let read = async {
        let mut response = Vec::new();
        let mut buf = [0; 1024];
        while !response.windows(2).any(|pair| pair == b"\r\n") {
            let len = stream.read(&mut buf).await?;
            anyhow::ensure!(len > 0, "the connection closed without a response");
            response.extend_from_slice(&buf[..len]);
        }
        let response = String::from_utf8_lossy(&response).into_owned();
        Ok(response.lines().next().unwrap_or_default().to_string())
    };
    tokio::time::timeout(Duration::from_secs(5), read).await?
}

async fn send(addr: SocketAddr, bytes: &[u8]) -> anyhow::Result<String> {
    status_line(&mut TcpStream::connect(addr).await?, bytes).await
}

#[tokio::test]
async fn http_port_filters_the_source_address_of_the_header() -> anyhow::Result<()> {
    let trusted_port = alloc_tcp_port().await?;
    let untrusted_port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();
    let mock = server
        .mock("GET", "/")
        .with_body("ok")
        .expect(3)
        .create_async()
        .await;

    let config = TestStorage::builder()
        .ports(vec![
            with_proxy_protocol(
                http_port_entry("trust", &trusted_port),
                receive(&["127.0.0.0/8", "::1/128"]),
            ),
            with_proxy_protocol(
                http_port_entry("other", &untrusted_port),
                receive(&["10.0.0.0/8"]),
            ),
        ])
        .proxies(vec![
            http_proxy_entry("proxy1", "trust", filtered_proxy(&upstream)),
            http_proxy_entry("proxy2", "other", filtered_proxy(&upstream)),
        ])
        .build();

    with_server(config, |_| async move {
        let addr = trusted_port.socket_addr();
        wait_for_listener(addr).await?;

        let denied = [v1_header(DENIED_SOURCE), REQUEST.to_vec()].concat();
        assert_eq!(send(addr, &denied).await?, "HTTP/1.1 403 Forbidden");
        let denied = [v2_header(DENIED_SOURCE), REQUEST.to_vec()].concat();
        assert_eq!(send(addr, &denied).await?, "HTTP/1.1 403 Forbidden");

        let allowed = [v1_header(ALLOWED_SOURCE), REQUEST.to_vec()].concat();
        assert_eq!(send(addr, &allowed).await?, "HTTP/1.1 200 OK");
        let allowed = [v2_header(ALLOWED_SOURCE), REQUEST.to_vec()].concat();
        assert_eq!(send(addr, &allowed).await?, "HTTP/1.1 200 OK");

        // A trusted peer must send the header.
        assert!(send(addr, REQUEST).await.is_err());

        // r3v3rs3 does not read a header from an untrusted peer.
        let addr = untrusted_port.socket_addr();
        wait_for_listener(addr).await?;
        assert_eq!(send(addr, REQUEST).await?, "HTTP/1.1 200 OK");
        Ok(())
    })
    .await?;

    mock.assert_async().await;
    Ok(())
}

/// Starts a TCP server that sends back every byte that it receives.
async fn start_echo_server() -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut reader, mut writer) = stream.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    Ok(addr)
}

#[tokio::test]
async fn tcp_port_passes_the_payload_after_the_header() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let echo = start_echo_server().await?;
    let proxy = TcpProxy {
        client_cert: None,
        connect_timeout: r3v3rs3_api::upstream::DEFAULT_CONNECT_TIMEOUT,
        load_balancing: Default::default(),
        health_check: Default::default(),
        circuit_breaker: Default::default(),
        upstream_servers: vec![UpstreamServer::new(
            format!("/ip4/127.0.0.1/tcp/{}", echo.port()).parse()?,
        )],
    };
    let config = TestStorage::builder()
        .ports(vec![with_proxy_protocol(
            port_entry("test", proxy_port.multiaddr_tcp()),
            receive(&["127.0.0.0/8", "::1/128"]),
        )])
        .proxies(vec![proxy_entry("proxy", "test", ProxyKind::Tcp(proxy))])
        .build();

    with_server(config, |_| async move {
        let addr = proxy_port.socket_addr();
        wait_for_listener(addr).await?;

        // A trusted peer that sends nothing does not delay other connections.
        let mut idle = TcpStream::connect(addr).await?;

        for header in [v1_header(ALLOWED_SOURCE), v2_header(ALLOWED_SOURCE)] {
            let mut stream = TcpStream::connect(addr).await?;
            stream
                .write_all(&[header, b"hello".to_vec()].concat())
                .await?;
            let mut echoed = [0; 5];
            tokio::time::timeout(Duration::from_millis(900), stream.read_exact(&mut echoed))
                .await??;
            assert_eq!(&echoed, b"hello");
        }

        // The idle connection closes when the header timeout expires.
        let mut buf = [0; 1];
        let read = tokio::time::timeout(Duration::from_secs(3), idle.read(&mut buf)).await?;
        assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
        Ok(())
    })
    .await
}

fn tls_connector(root: &Cert) -> anyhow::Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut &root.pem_chain[..]) {
        roots.add(cert?)?;
    }
    let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

#[tokio::test]
async fn https_port_reads_the_header_before_the_tls_handshake() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();
    let mock = server
        .mock("GET", "/")
        .with_body("ok")
        .expect(1)
        .create_async()
        .await;

    let root = Arc::new(Cert::new_ca()?);
    let cert = Arc::new(Cert::new_self_signed(&["localhost".parse()?], &root)?);
    let mut entry = with_proxy_protocol(
        port_entry("test", proxy_port.multiaddr_https()),
        receive(&["127.0.0.0/8", "::1/128"]),
    );
    entry.port.opts.tls_termination = Some(TlsTermination {
        server_names: vec!["localhost".into()],
        ..Default::default()
    });
    let config = TestStorage::builder()
        .ports(vec![entry])
        .proxies(vec![http_proxy_entry(
            "proxy",
            "test",
            filtered_proxy(&upstream),
        )])
        .certs(
            [(root.id, root.clone()), (cert.id, cert.clone())]
                .into_iter()
                .collect(),
        )
        .build();
    let connector = tls_connector(&root)?;

    with_server(config, |_| async move {
        let addr = proxy_port.socket_addr();
        wait_for_listener(addr).await?;
        for (source, expected) in [
            (ALLOWED_SOURCE, "HTTP/1.1 200 OK"),
            (DENIED_SOURCE, "HTTP/1.1 403 Forbidden"),
        ] {
            let mut stream = TcpStream::connect(addr).await?;
            stream.write_all(&v1_header(source)).await?;
            let name = ServerName::try_from("localhost")?;
            let mut tls = connector.connect(name, stream).await?;
            assert_eq!(status_line(&mut tls, REQUEST).await?, expected);
        }
        Ok(())
    })
    .await?;

    mock.assert_async().await;
    Ok(())
}
