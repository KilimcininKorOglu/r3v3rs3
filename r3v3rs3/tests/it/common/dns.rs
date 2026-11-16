//! An in-process DNS server over UDP that answers SRV queries from a table.

use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{Name, RData, Record, rdata::SRV},
    serialize::binary::{BinDecodable, BinEncodable},
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::net::UdpSocket;

/// One SRV record: `priority weight port target`.
#[derive(Debug, Clone)]
pub struct SrvRecord {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

impl SrvRecord {
    pub fn new(port: u16, target: &str) -> Self {
        Self {
            priority: 0,
            weight: 0,
            port,
            target: target.into(),
        }
    }
}

/// The answer for one name.
#[derive(Debug, Clone)]
enum Answer {
    /// The SRV records with their TTL.
    Records(u32, Vec<SrvRecord>),
    /// SERVFAIL, a failed lookup.
    ServerFailure,
}

type Table = Arc<Mutex<HashMap<String, Answer>>>;

pub struct MockDns {
    pub addr: SocketAddr,
    table: Table,
}

impl MockDns {
    /// Binds a local UDP port and answers the queries in a task.
    pub async fn start() -> anyhow::Result<Self> {
        let port = super::alloc_udp_port().await?;
        let socket = UdpSocket::bind(port.socket_addr()).await?;
        let addr = socket.local_addr()?;
        let table: Table = Default::default();
        let answers = table.clone();
        tokio::spawn(async move {
            let mut buf = [0; 4096];
            while let Ok((len, peer)) = socket.recv_from(&mut buf).await {
                if let Some(response) = answer(&answers, &buf[..len]) {
                    let _ = socket.send_to(&response, peer).await;
                }
            }
        });
        Ok(Self { addr, table })
    }

    /// Sets the SRV records and the TTL of a name.
    pub fn set(&self, name: &str, ttl: u32, records: Vec<SrvRecord>) {
        self.table
            .lock()
            .unwrap()
            .insert(name.to_lowercase(), Answer::Records(ttl, records));
    }

    /// Answers the queries of a name with SERVFAIL.
    pub fn fail(&self, name: &str) {
        self.table
            .lock()
            .unwrap()
            .insert(name.to_lowercase(), Answer::ServerFailure);
    }
}

/// Builds the response of a query: the SRV records of the name, or NXDOMAIN.
fn answer(table: &Table, query: &[u8]) -> Option<Vec<u8>> {
    let request = Message::from_bytes(query).ok()?;
    let question = request.queries.first()?.clone();
    let name = question
        .name()
        .to_utf8()
        .trim_end_matches('.')
        .to_lowercase();
    let mut response = Message::new(
        request.metadata.id,
        MessageType::Response,
        request.metadata.op_code,
    );
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.queries.push(question.clone());
    let answer = table.lock().unwrap().get(&name).cloned();
    match answer {
        Some(Answer::Records(ttl, records)) => {
            for record in records {
                // "." is the root name, the target of a service that is not available.
                let target = match record.target.as_str() {
                    "." => Name::root(),
                    host => Name::from_str(&format!("{host}.")).ok()?,
                };
                let srv = SRV::new(record.priority, record.weight, record.port, target);
                response.answers.push(Record::from_rdata(
                    question.name().clone(),
                    ttl,
                    RData::SRV(srv),
                ));
            }
        }
        Some(Answer::ServerFailure) => {
            response.metadata.response_code = ResponseCode::ServFail;
        }
        None => {
            response.metadata.response_code = ResponseCode::NXDomain;
        }
    }
    response.to_bytes().ok()
}
