//! Reads the PROXY protocol header that a trusted load balancer sends before the client data, and
//! builds the header that a TCP proxy sends to its upstream servers.

use anyhow::{anyhow, bail, ensure};
use ppp::{v1, v2};
use r3v3rs3_api::proxy_protocol::{
    ProxyProtocolAccept, ProxyProtocolReceive, ProxyProtocolVersion,
};
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncReadExt, BufStream};
use tokio::net::TcpStream;

/// The signature, the version, the family and the length of a v2 header.
const V2_FIXED_LEN: usize = 16;
/// The largest address block of a v2 header that r3v3rs3 reads.
const V2_MAX_ADDRESS_LEN: usize = 4096;
const V1_PREFIX: &[u8] = b"PROXY ";
/// The largest v1 header, including the line break.
const V1_MAX_LEN: usize = 107;

/// An accepted client connection.
pub struct Client {
    /// The stream after the PROXY protocol header.
    pub stream: BufStream<TcpStream>,
    /// The client address from the header, or the peer address.
    pub remote: SocketAddr,
    /// The address of the connected peer.
    pub peer: SocketAddr,
}

/// Builds the header of a TCP connection from the client address to the address that the client
/// connected to. When the families differ, both addresses become IPv6 addresses.
pub fn header(
    version: ProxyProtocolVersion,
    source: SocketAddr,
    destination: SocketAddr,
) -> std::io::Result<Vec<u8>> {
    let (source, destination) = if source.is_ipv4() == destination.is_ipv4() {
        (source, destination)
    } else {
        (ipv6(source), ipv6(destination))
    };
    match version {
        ProxyProtocolVersion::V1 => {
            let family = if source.is_ipv4() { "TCP4" } else { "TCP6" };
            let line = format!(
                "PROXY {family} {} {} {} {}\r\n",
                source.ip(),
                destination.ip(),
                source.port(),
                destination.port()
            );
            Ok(line.into_bytes())
        }
        ProxyProtocolVersion::V2 => v2::Builder::with_addresses(
            v2::Version::Two | v2::Command::Proxy,
            v2::Protocol::Stream,
            (source, destination),
        )
        .build(),
    }
}

/// The v2 `LOCAL` header of a connection that carries no client, for example a health check.
pub fn local_header() -> std::io::Result<Vec<u8>> {
    v2::Builder::new(
        v2::Version::Two | v2::Command::Local,
        v2::AddressFamily::Unspecified | v2::Protocol::Unspecified,
    )
    .build()
}

fn ipv6(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V4(addr) => (addr.ip().to_ipv6_mapped(), addr.port()).into(),
        SocketAddr::V6(_) => addr,
    }
}

/// Reads the client address of a new connection.
pub async fn accept(
    mut stream: BufStream<TcpStream>,
    config: Option<&ProxyProtocolReceive>,
) -> anyhow::Result<Client> {
    let peer = stream.get_ref().peer_addr()?;
    let remote = read_source(&mut stream, peer, config).await?;
    Ok(Client {
        stream,
        remote,
        peer,
    })
}

/// Returns the source address of the header of a trusted peer. It reads exactly the header, so the
/// client data stays in the stream. Another peer is not read, and its address is returned.
async fn read_source<S>(
    stream: &mut S,
    peer: SocketAddr,
    config: Option<&ProxyProtocolReceive>,
) -> anyhow::Result<SocketAddr>
where
    S: AsyncRead + Unpin,
{
    let Some(config) = config.filter(|config| config.trusts(peer.ip())) else {
        return Ok(peer);
    };
    let header = tokio::time::timeout(config.timeout, read_header(stream))
        .await
        .map_err(|_| {
            anyhow!(
                "no PROXY protocol header from {peer} in {:?}",
                config.timeout
            )
        })??;
    source(&header, config.accept, peer)
}

async fn read_header<S>(stream: &mut S) -> anyhow::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut header = vec![0; v2::PROTOCOL_PREFIX.len()];
    stream.read_exact(&mut header).await?;
    if header == v2::PROTOCOL_PREFIX {
        read_v2_rest(stream, header).await
    } else if header.starts_with(V1_PREFIX) {
        read_v1_rest(stream, header).await
    } else {
        bail!("the connection does not start with a PROXY protocol header")
    }
}

async fn read_v2_rest<S>(stream: &mut S, mut header: Vec<u8>) -> anyhow::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let start = header.len();
    header.resize(V2_FIXED_LEN, 0);
    stream.read_exact(&mut header[start..]).await?;
    let len = usize::from(u16::from_be_bytes([header[14], header[15]]));
    ensure!(
        len <= V2_MAX_ADDRESS_LEN,
        "the PROXY protocol v2 address block is longer than {V2_MAX_ADDRESS_LEN} bytes"
    );
    header.resize(V2_FIXED_LEN + len, 0);
    stream.read_exact(&mut header[V2_FIXED_LEN..]).await?;
    Ok(header)
}

async fn read_v1_rest<S>(stream: &mut S, mut header: Vec<u8>) -> anyhow::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    while !header.ends_with(b"\r\n") {
        ensure!(
            header.len() < V1_MAX_LEN,
            "the PROXY protocol v1 header is longer than {V1_MAX_LEN} bytes"
        );
        header.push(stream.read_u8().await?);
    }
    Ok(header)
}

fn source(
    header: &[u8],
    accept: ProxyProtocolAccept,
    peer: SocketAddr,
) -> anyhow::Result<SocketAddr> {
    if header.starts_with(v2::PROTOCOL_PREFIX) {
        ensure!(
            accept != ProxyProtocolAccept::V1,
            "the port does not accept PROXY protocol v2"
        );
        v2_source(header, peer)
    } else {
        ensure!(
            accept != ProxyProtocolAccept::V2,
            "the port does not accept PROXY protocol v1"
        );
        v1_source(header, peer)
    }
}

/// A `LOCAL` header, for example a health check of the load balancer, keeps the peer address.
fn v2_source(header: &[u8], peer: SocketAddr) -> anyhow::Result<SocketAddr> {
    let header = v2::Header::try_from(header)
        .map_err(|err| anyhow!("invalid PROXY protocol v2 header: {err}"))?;
    if header.command == v2::Command::Local {
        return Ok(peer);
    }
    Ok(match header.addresses {
        v2::Addresses::IPv4(addr) => (addr.source_address, addr.source_port).into(),
        v2::Addresses::IPv6(addr) => (addr.source_address, addr.source_port).into(),
        v2::Addresses::Unspecified | v2::Addresses::Unix(_) => peer,
    })
}

/// An `UNKNOWN` header keeps the peer address.
fn v1_source(header: &[u8], peer: SocketAddr) -> anyhow::Result<SocketAddr> {
    let header = v1::Header::try_from(header)
        .map_err(|err| anyhow!("invalid PROXY protocol v1 header: {err}"))?;
    Ok(match header.addresses {
        v1::Addresses::Tcp4(addr) => (addr.source_address, addr.source_port).into(),
        v1::Addresses::Tcp6(addr) => (addr.source_address, addr.source_port).into(),
        v1::Addresses::Unknown => peer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const PEER: &str = "127.0.0.1:40000";

    fn config(accept: ProxyProtocolAccept) -> ProxyProtocolReceive {
        ProxyProtocolReceive {
            accept,
            trusted: vec!["127.0.0.0/8".parse().unwrap()],
            timeout: Duration::from_secs(1),
        }
    }

    /// Reads the source from the bytes, and returns it with the bytes that stay unread.
    async fn read(
        bytes: &[u8],
        config: &ProxyProtocolReceive,
    ) -> (anyhow::Result<SocketAddr>, Vec<u8>) {
        let mut reader = bytes;
        let result = read_source(&mut reader, PEER.parse().unwrap(), Some(config)).await;
        (result, reader.to_vec())
    }

    fn v2_header(command: v2::Command, source: &str) -> Vec<u8> {
        let source: SocketAddr = source.parse().unwrap();
        let destination: SocketAddr = if source.is_ipv4() {
            "127.0.0.1:80".parse().unwrap()
        } else {
            "[::1]:80".parse().unwrap()
        };
        v2::Builder::with_addresses(
            v2::Version::Two | command,
            v2::Protocol::Stream,
            (source, destination),
        )
        .build()
        .unwrap()
    }

    #[tokio::test]
    async fn a_built_header_reads_back_as_the_same_addresses() {
        let config = config(ProxyProtocolAccept::Any);
        let cases = [
            ("203.0.113.9:51000", "127.0.0.1:8080", "203.0.113.9:51000"),
            ("[2001:db8::1]:51000", "[::1]:8080", "[2001:db8::1]:51000"),
            (
                "203.0.113.9:51000",
                "[::1]:8080",
                "[::ffff:203.0.113.9]:51000",
            ),
        ];
        for version in [ProxyProtocolVersion::V1, ProxyProtocolVersion::V2] {
            for (source, destination, expected) in cases {
                let bytes = header(
                    version,
                    source.parse().unwrap(),
                    destination.parse().unwrap(),
                )
                .unwrap();
                let (result, rest) = read(&bytes, &config).await;
                assert_eq!(result.unwrap(), expected.parse().unwrap(), "{version:?}");
                assert!(rest.is_empty());
            }
        }

        let local = local_header().unwrap();
        let (result, rest) = read(&local, &config).await;
        assert_eq!(result.unwrap(), PEER.parse().unwrap());
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn an_untrusted_peer_is_not_read() {
        let untrusted = ProxyProtocolReceive {
            trusted: vec!["10.0.0.0/8".parse().unwrap()],
            ..config(ProxyProtocolAccept::Any)
        };
        let bytes = b"PROXY TCP4 203.0.113.9 127.0.0.1 51000 80\r\nGET";
        let (result, rest) = read(bytes, &untrusted).await;
        assert_eq!(result.unwrap(), PEER.parse().unwrap());
        assert_eq!(rest, bytes);

        let mut reader = &bytes[..];
        let result = read_source(&mut reader, PEER.parse().unwrap(), None).await;
        assert_eq!(result.unwrap(), PEER.parse().unwrap());
        assert_eq!(reader, bytes);
    }

    #[tokio::test]
    async fn a_v1_header_gives_the_source_and_keeps_the_payload() {
        let config = config(ProxyProtocolAccept::Any);
        let (result, rest) =
            read(b"PROXY TCP4 203.0.113.9 127.0.0.1 51000 80\r\nGET", &config).await;
        assert_eq!(result.unwrap(), "203.0.113.9:51000".parse().unwrap());
        assert_eq!(rest, b"GET");

        let (result, rest) = read(b"PROXY TCP6 2001:db8::1 ::1 51000 80\r\n", &config).await;
        assert_eq!(result.unwrap(), "[2001:db8::1]:51000".parse().unwrap());
        assert!(rest.is_empty());

        let (result, _) = read(b"PROXY UNKNOWN\r\n", &config).await;
        assert_eq!(result.unwrap(), PEER.parse().unwrap());
    }

    #[tokio::test]
    async fn a_v2_header_gives_the_source_and_keeps_the_payload() {
        let config = config(ProxyProtocolAccept::Any);
        for source in ["203.0.113.9:51000", "[2001:db8::1]:51000"] {
            let mut bytes = v2_header(v2::Command::Proxy, source);
            bytes.extend_from_slice(b"payload");
            let (result, rest) = read(&bytes, &config).await;
            assert_eq!(result.unwrap(), source.parse().unwrap());
            assert_eq!(rest, b"payload");
        }

        let local = v2_header(v2::Command::Local, "203.0.113.9:51000");
        let (result, _) = read(&local, &config).await;
        assert_eq!(result.unwrap(), PEER.parse().unwrap());
    }

    #[tokio::test]
    async fn a_trusted_peer_without_a_valid_header_is_an_error() {
        let config = config(ProxyProtocolAccept::Any);
        for bytes in [
            &b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"[..],
            b"PROXY TCP4 203.0.113.9",
            b"PROXY TCP4 not-an-address 127.0.0.1 51000 80\r\n",
        ] {
            let (result, _) = read(bytes, &config).await;
            assert!(result.is_err(), "{}", String::from_utf8_lossy(bytes));
        }

        let long = [b"PROXY TCP4 ".as_slice(), &[b'1'; 120]].concat();
        let (result, rest) = read(&long, &config).await;
        assert!(result.unwrap_err().to_string().contains("longer than 107"));
        assert!(!rest.is_empty());

        let mut oversized = v2_header(v2::Command::Proxy, "203.0.113.9:51000");
        oversized[14..16].copy_from_slice(&4097u16.to_be_bytes());
        let (result, rest) = read(&oversized, &config).await;
        assert!(result.unwrap_err().to_string().contains("longer than 4096"));
        assert_eq!(rest.len(), oversized.len() - V2_FIXED_LEN);
    }

    #[tokio::test]
    async fn the_port_rejects_a_version_that_it_does_not_accept() {
        let v2 = v2_header(v2::Command::Proxy, "203.0.113.9:51000");
        let (result, _) = read(&v2, &config(ProxyProtocolAccept::V1)).await;
        assert!(result.unwrap_err().to_string().contains("v2"));

        let v1 = b"PROXY TCP4 203.0.113.9 127.0.0.1 51000 80\r\n";
        let (result, _) = read(v1, &config(ProxyProtocolAccept::V2)).await;
        assert!(result.unwrap_err().to_string().contains("v1"));

        let (result, _) = read(v1, &config(ProxyProtocolAccept::V1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_slow_header_times_out() {
        let config = ProxyProtocolReceive {
            timeout: Duration::from_millis(50),
            ..config(ProxyProtocolAccept::Any)
        };
        let (_client, mut server) = tokio::io::duplex(64);
        let result = read_source(&mut server, PEER.parse().unwrap(), Some(&config)).await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no PROXY protocol header"));
    }
}
