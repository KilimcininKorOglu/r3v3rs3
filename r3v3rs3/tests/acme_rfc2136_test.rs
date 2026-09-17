use base64::{engine::general_purpose::STANDARD, Engine};
use hickory_proto::{
    op::{Message, MessageType, OpCode, ResponseCode},
    rr::{
        rdata::{tsig::TsigAlgorithm, SOA},
        Name, RData, Record, TSigResponseContext, TSigner,
    },
};
use r3v3rs3::certs::dns::{self, DnsClient, TxtName};
use r3v3rs3_api::{
    acme::{DnsProvider, LocalProvider, TsigKeyAlgorithm},
    app::AcmeExecConfig,
};
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const KEY_NAME: &str = "r3v3rs3-key.";

type Received = Arc<Mutex<Vec<String>>>;

/// How the mock DNS server answers.
#[derive(Clone, Copy)]
enum Mode {
    Accept,
    RefuseUpdates,
    /// Signs every response with another key.
    WrongKey,
}

fn signer(key: Vec<u8>) -> anyhow::Result<TSigner> {
    Ok(TSigner::new(
        key,
        TsigAlgorithm::HmacSha256,
        Name::from_ascii(KEY_NAME)?,
        300,
    )?)
}

/// Starts a DNS server over TCP that verifies the TSIG of each request and signs its response.
async fn start_server(key: Vec<u8>, mode: Mode) -> anyhow::Result<(String, Received)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?.to_string();
    let received = Received::default();
    let log = received.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            if let Err(err) = serve(stream, &key, mode, &log).await {
                log.lock().unwrap().push(format!("server error: {err}"));
            }
        }
    });
    Ok((addr, received))
}

async fn serve(
    mut stream: TcpStream,
    key: &[u8],
    mode: Mode,
    log: &Received,
) -> anyhow::Result<()> {
    let length = stream.read_u16().await?;
    let mut bytes = vec![0; usize::from(length)];
    stream.read_exact(&mut bytes).await?;
    let (request_mac, _, _) = signer(key.to_vec())?.verify_message_byte(&bytes, None, true)?;
    let request = Message::from_vec(&bytes)?;
    log.lock().unwrap().push(describe(&request));

    let mut response = Message::new(
        request.metadata.id,
        MessageType::Response,
        request.metadata.op_code,
    );
    response.queries.clone_from(&request.queries);
    match (request.metadata.op_code, mode) {
        (OpCode::Query, _) => {
            let soa = SOA::new(
                Name::from_ascii("ns1.example.test.")?,
                Name::from_ascii("admin.example.test.")?,
                1,
                3600,
                600,
                86400,
                60,
            );
            let zone = Name::from_ascii("example.test.")?;
            response.metadata.response_code = ResponseCode::NXDomain;
            response
                .authorities
                .push(Record::from_rdata(zone, 60, RData::SOA(soa)));
        }
        (_, Mode::RefuseUpdates) => {
            response.metadata.response_code = ResponseCode::Refused;
        }
        _ => {}
    }
    let signing_key = match mode {
        Mode::WrongKey => b"another key".to_vec(),
        _ => key.to_vec(),
    };
    sign_response(&mut response, &request_mac, signing_key)?;

    let bytes = response.to_vec()?;
    stream
        .write_all(&u16::try_from(bytes.len())?.to_be_bytes())
        .await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

fn sign_response(response: &mut Message, request_mac: &[u8], key: Vec<u8>) -> anyhow::Result<()> {
    let signer = signer(key)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    // The MAC covers the wire bytes of the response, so the message is encoded before it is signed.
    let wire = response.to_vec()?;
    let context =
        TSigResponseContext::new(response.metadata.id, now, signer, request_mac.to_vec(), None);
    response.signature = Some(context.sign(&wire)?);
    Ok(())
}

/// The op code, the zone or query, and the update records of a request.
fn describe(request: &Message) -> String {
    let query = request
        .queries
        .first()
        .map(|query| format!("{} {}", query.name(), query.query_type()))
        .unwrap_or_default();
    let updates = request
        .authorities
        .iter()
        .map(|record| {
            let values = match &record.data {
                RData::TXT(txt) => txt
                    .txt_data
                    .iter()
                    .map(|data| String::from_utf8_lossy(data).into_owned())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => String::new(),
            };
            format!(
                "{} {:?} {} {} {values}",
                record.name,
                record.dns_class,
                record.record_type(),
                record.ttl
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{:?} {query} [{}]",
        request.metadata.op_code,
        updates.join("; ")
    )
}

fn provider(server: &str, zone: &str, key: &[u8]) -> DnsProvider {
    DnsProvider::Local(LocalProvider::Rfc2136 {
        server: server.to_string(),
        zone: zone.to_string(),
        key_name: "r3v3rs3-key".to_string(),
        key_algorithm: TsigKeyAlgorithm::HmacSha256,
        key_secret: STANDARD.encode(key),
    })
}

fn challenge_name() -> TxtName {
    TxtName {
        fqdn: "_acme-challenge.app.example.test".to_string(),
        values: vec!["v1".to_string(), "v2".to_string()],
    }
}

/// Starts a mock DNS server with a new key, and returns a client of the provider for it.
async fn mock_client(mode: Mode, zone: &str) -> anyhow::Result<(Box<dyn DnsClient>, Received)> {
    let key = rand::random::<[u8; 32]>().to_vec();
    let (server, received) = start_server(key.clone(), mode).await?;
    let client = dns::client(&provider(&server, zone, &key), &AcmeExecConfig::default()).await?;
    Ok((client, received))
}

/// The error of adding the challenge name, and the requests that the server received.
async fn add_error(mode: Mode, zone: &str) -> anyhow::Result<(String, Received)> {
    let (client, received) = mock_client(mode, zone).await?;
    let Err(err) = client.add_txt(&challenge_name()).await else {
        anyhow::bail!("add_txt succeeded");
    };
    Ok((format!("{err:#}"), received))
}

#[tokio::test]
async fn rfc2136_finds_the_zone_and_signs_every_update() -> anyhow::Result<()> {
    let (client, received) = mock_client(Mode::Accept, "").await?;

    let record = client.add_txt(&challenge_name()).await?;
    assert_eq!(record.zone, "example.test.");
    client.remove_txt(&record).await?;

    let name = "_acme-challenge.app.example.test.";
    assert_eq!(
        *received.lock().unwrap(),
        vec![
            format!("Query {name} SOA []"),
            format!("Update example.test. SOA [{name} IN TXT 60 v1; {name} IN TXT 60 v2]"),
            format!("Update example.test. SOA [{name} NONE TXT 0 v1; {name} NONE TXT 0 v2]"),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn rfc2136_rejects_a_response_signed_with_another_key() -> anyhow::Result<()> {
    let (err, _) = add_error(Mode::WrongKey, "example.test").await?;
    assert!(err.contains("TSIG signature"), "{err}");
    Ok(())
}

#[tokio::test]
async fn rfc2136_reports_a_refused_update() -> anyhow::Result<()> {
    let (err, _) = add_error(Mode::RefuseUpdates, "example.test").await?;
    assert!(err.contains("Refused"), "{err}");
    Ok(())
}

#[tokio::test]
async fn rfc2136_refuses_a_name_outside_the_zone_without_sending_it() -> anyhow::Result<()> {
    let (err, received) = add_error(Mode::Accept, "other.test").await?;
    assert!(err.contains("is not in the zone"), "{err}");
    assert!(received.lock().unwrap().is_empty());
    Ok(())
}
