//! An in-process DNS server over UDP that answers SRV queries from a table.

use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{rdata::SRV, Name, RData, Record},
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

type Table = Arc<Mutex<HashMap<String, (u32, Vec<SrvRecord>)>>>;

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
            .insert(name.to_lowercase(), (ttl, records));
    }
}

/// Builds the response of a query: the SRV records of the name, or NXDOMAIN.
fn answer(table: &Table, query: &[u8]) -> Option<Vec<u8>> {
    let request = Message::from_bytes(query).ok()?;
    let question = request.queries().first()?.clone();
    let name = question
        .name()
        .to_utf8()
        .trim_end_matches('.')
        .to_lowercase();
    let mut response = Message::new();
    response
        .set_id(request.id())
        .set_message_type(MessageType::Response)
        .set_op_code(request.op_code())
        .set_recursion_desired(request.recursion_desired())
        .set_recursion_available(true)
        .add_query(question.clone());
    match table.lock().unwrap().get(&name) {
        Some((ttl, records)) => {
            for record in records {
                let target = Name::from_str(&format!("{}.", record.target)).ok()?;
                let srv = SRV::new(record.priority, record.weight, record.port, target);
                response.add_answer(Record::from_rdata(
                    question.name().clone(),
                    *ttl,
                    RData::SRV(srv),
                ));
            }
        }
        None => {
            response.set_response_code(ResponseCode::NXDomain);
        }
    }
    response.to_bytes().ok()
}
