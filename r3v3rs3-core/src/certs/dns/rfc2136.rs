use super::{DnsClient, TXT_TTL, TxtName, TxtRecord};
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use hickory_proto::{
    op::{Message, MessageType, OpCode, Query, ResponseCode, update_message},
    rr::{
        Name, RData, RecordSet, RecordType, TSigner,
        rdata::{TXT, tsig::TsigAlgorithm},
    },
};
use r3v3rs3_api::acme::TsigKeyAlgorithm;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// Longest wait for the connection and the response of one DNS message.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Allowed clock difference between r3v3rs3 and the DNS server, in seconds.
const TSIG_FUDGE: u16 = 300;

/// Sends RFC 2136 dynamic updates signed with TSIG over TCP.
pub struct Rfc2136 {
    server: String,
    zone: Option<Name>,
    signer: TSigner,
}

impl Rfc2136 {
    /// An empty `zone` asks the server for the SOA record of each name.
    pub fn new(
        server: &str,
        zone: &str,
        key_name: &str,
        algorithm: TsigKeyAlgorithm,
        secret: &str,
    ) -> anyhow::Result<Self> {
        let key = STANDARD
            .decode(secret.trim())
            .map_err(|_| anyhow!("the TSIG secret is not base64"))?;
        let signer = TSigner::new(
            key,
            tsig_algorithm(algorithm),
            fqdn_name(key_name)?,
            TSIG_FUDGE,
        )?;
        let zone = match zone.trim() {
            "" => None,
            zone => Some(fqdn_name(zone)?),
        };
        Ok(Self {
            server: server.trim().to_string(),
            zone,
            signer,
        })
    }

    /// Signs the message, sends it, and returns the response after its TSIG is verified.
    async fn exchange(&self, mut message: Message) -> anyhow::Result<Message> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let mut verifier = message
            .finalize(&self.signer, now)?
            .ok_or_else(|| anyhow!("TSIG signing returned no verifier"))?;
        let request = message.to_vec()?;
        let response = tokio::time::timeout(EXCHANGE_TIMEOUT, send_tcp(&self.server, &request))
            .await
            .map_err(|_| anyhow!("the DNS server {} did not answer", self.server))??;

        self.check_response(&response, message.metadata.id)?;
        let verified = verifier.verify(&response).map_err(|err| {
            anyhow!(
                "the TSIG signature of the DNS server {} is invalid: {err}",
                self.server
            )
        })?;
        Ok(verified.into_message())
    }

    /// Rejects a response of another message and a TSIG error. A server answers a TSIG error, for
    /// example an unknown key, without a signature.
    fn check_response(&self, response: &[u8], request_id: u16) -> anyhow::Result<()> {
        let unsigned = Message::from_vec(response)?;
        if unsigned.metadata.id != request_id {
            bail!("the DNS server {} answered another message", self.server);
        }
        if unsigned.signature.is_none() && unsigned.metadata.response_code != ResponseCode::NoError
        {
            bail!(
                "the DNS server {} answered {}",
                self.server,
                unsigned.metadata.response_code
            );
        }
        Ok(())
    }

    /// The zone of `fqdn`: the configured zone, or the owner of the SOA record that the server
    /// returns for `fqdn`.
    async fn zone(&self, fqdn: &Name) -> anyhow::Result<Name> {
        let zone = match &self.zone {
            Some(zone) => zone.clone(),
            None => self.soa_owner(fqdn).await?,
        };
        check_zone(&zone, fqdn)?;
        Ok(zone)
    }

    async fn soa_owner(&self, fqdn: &Name) -> anyhow::Result<Name> {
        let mut message = Message::new(rand::random(), MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = false;
        message
            .queries
            .push(Query::query(fqdn.clone(), RecordType::SOA));
        let response = self.exchange(message).await?;
        // A name without records has the SOA record of its zone in the authority section.
        response
            .answers
            .iter()
            .chain(response.authorities.iter())
            .find(|record| record.record_type() == RecordType::SOA)
            .map(|record| record.name.clone())
            .ok_or_else(|| {
                anyhow!(
                    "the DNS server {} returned no SOA record for {fqdn}",
                    self.server
                )
            })
    }

    async fn update(&self, message: Message) -> anyhow::Result<()> {
        let response = self.exchange(message).await?;
        match response.metadata.response_code {
            ResponseCode::NoError => Ok(()),
            code => bail!("the DNS server {} refused the update: {code}", self.server),
        }
    }
}

#[async_trait]
impl DnsClient for Rfc2136 {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        let fqdn = fqdn_name(&name.fqdn)?;
        let zone = self.zone(&fqdn).await?;
        // The server applies an update as a whole, so a failed update adds no value.
        let message =
            update_message::append(txt_rrset(&fqdn, &name.values), zone.clone(), false, false);
        self.update(message).await?;
        Ok(TxtRecord::new(name, zone.to_ascii()))
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        let fqdn = fqdn_name(&record.fqdn)?;
        let zone = fqdn_name(&record.zone)?;
        check_zone(&zone, &fqdn)?;
        // Other TXT values of the name stay.
        let message =
            update_message::delete_by_rdata(txt_rrset(&fqdn, &record.values), zone, false);
        self.update(message).await
    }
}

/// `update_message` panics on a name outside the zone, so every update checks it first.
fn check_zone(zone: &Name, fqdn: &Name) -> anyhow::Result<()> {
    if !zone.zone_of(fqdn) {
        bail!("{fqdn} is not in the zone {zone}");
    }
    Ok(())
}

fn fqdn_name(name: &str) -> anyhow::Result<Name> {
    let mut parsed =
        Name::from_ascii(name.trim()).with_context(|| format!("invalid DNS name {name}"))?;
    parsed.set_fqdn(true);
    Ok(parsed)
}

fn txt_rrset(fqdn: &Name, values: &[String]) -> RecordSet {
    let mut rrset = RecordSet::with_ttl(fqdn.clone(), RecordType::TXT, TXT_TTL);
    for value in values {
        rrset.add_rdata(RData::TXT(TXT::new(vec![value.clone()])));
    }
    rrset
}

fn tsig_algorithm(algorithm: TsigKeyAlgorithm) -> TsigAlgorithm {
    match algorithm {
        TsigKeyAlgorithm::HmacSha256 => TsigAlgorithm::HmacSha256,
        TsigKeyAlgorithm::HmacSha384 => TsigAlgorithm::HmacSha384,
        TsigKeyAlgorithm::HmacSha512 => TsigAlgorithm::HmacSha512,
    }
}

/// Sends one DNS message over TCP with its length prefix and reads one response.
async fn send_tcp(server: &str, request: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(server)
        .await
        .with_context(|| format!("failed to connect to the DNS server {server}"))?;
    let mut framed = u16::try_from(request.len())?.to_be_bytes().to_vec();
    framed.extend_from_slice(request);
    stream.write_all(&framed).await?;
    let length = stream.read_u16().await?;
    let mut response = vec![0; usize::from(length)];
    stream.read_exact(&mut response).await?;
    Ok(response)
}
