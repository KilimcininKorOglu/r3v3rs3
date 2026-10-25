use r3v3rs3_api::early_data::{
    AntiReplayBuffer, EARLY_DATA_HEADER, EARLY_DATA_VALUE, is_safe_http_method,
};
use r3v3rs3_api::port::{Port, PortEntry, PortOptions};
use r3v3rs3_api::tls::TlsTermination;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

mod common;
use common::{TestStorage, alloc_tcp_port, with_server};

#[test]
fn test_safe_http_methods_per_rfc7231() {
    assert!(is_safe_http_method("GET"));
    assert!(is_safe_http_method("HEAD"));
    assert!(is_safe_http_method("OPTIONS"));
    assert!(is_safe_http_method("TRACE"));

    assert!(!is_safe_http_method("POST"));
    assert!(!is_safe_http_method("PUT"));
    assert!(!is_safe_http_method("DELETE"));
    assert!(!is_safe_http_method("PATCH"));
    assert!(!is_safe_http_method("CONNECT"));
}

#[test]
fn test_anti_replay_buffer_logic() {
    let buffer = AntiReplayBuffer::new(Duration::from_secs(30), 500);
    let client_ip: IpAddr = Ipv4Addr::new(192, 168, 1, 100).into();

    // 1. Initial 0-RTT request is accepted
    assert!(buffer.check_and_record(client_ip, "GET", "/resource"));

    // 2. Immediate replay of exact same request is rejected
    assert!(!buffer.check_and_record(client_ip, "GET", "/resource"));

    // 3. Different URI is accepted
    assert!(buffer.check_and_record(client_ip, "GET", "/resource?query=1"));

    // 4. Different IP is accepted
    let client_ip_2: IpAddr = Ipv4Addr::new(192, 168, 1, 101).into();
    assert!(buffer.check_and_record(client_ip_2, "GET", "/resource"));
}

#[test]
fn test_tls_termination_early_data_serialization() {
    let mut tls = TlsTermination {
        server_names: vec!["example.com".into()],
        early_data: true,
        ..Default::default()
    };

    let serialized = serde_json::to_string(&tls).unwrap();
    assert!(serialized.contains(r#""early_data":true"#));

    let deserialized: TlsTermination = serde_json::from_str(&serialized).unwrap();
    assert!(deserialized.early_data);

    tls.early_data = false;
    let serialized_off = serde_json::to_string(&tls).unwrap();
    assert!(!serialized_off.contains("early_data"));
}

#[tokio::test]
async fn test_port_options_with_early_data() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let config = TestStorage::builder()
        .ports(vec![PortEntry {
            id: "test".parse().unwrap(),
            port: Port {
                active: true,
                name: "secure-port".into(),
                listen: proxy_port.multiaddr_https(),
                opts: PortOptions {
                    tls_termination: Some(TlsTermination {
                        server_names: vec!["localhost".into()],
                        early_data: true,
                        ..Default::default()
                    }),
                    proxy_protocol: None,
                    http2_limits: None,
                    http3_limits: None,
                    stream_priority: None,
                },
            },
        }])
        .build();

    with_server(config, |_| async move {
        assert_eq!(EARLY_DATA_HEADER, "early-data");
        assert_eq!(EARLY_DATA_VALUE, "1");
        Ok(())
    })
    .await
}
