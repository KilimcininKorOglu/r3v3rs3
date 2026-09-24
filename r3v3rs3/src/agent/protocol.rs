//! The messages of the agent link. The set is closed: the agent never receives a program name,
//! a shell string or raw arguments, and every operand is a type that validates itself.

use r3v3rs3_api::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::fmt;

/// The only frame of a connection without a client certificate.
#[derive(Clone, Serialize, Deserialize)]
pub struct EnrollRequest {
    /// The secret of the enrollment token.
    pub secret: String,
    /// The certificate signing request of the new agent key in PEM.
    pub csr: String,
    /// The version of the agent.
    pub version: String,
}

// The secret stays out of every log line.
impl fmt::Debug for EnrollRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollRequest")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum EnrollResponse {
    Enrolled(Enrolled),
    Refused { message: String },
}

/// The identity of an enrolled agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrolled {
    pub target: ShortId,
    /// The client certificate of the agent in PEM.
    pub certificate: String,
    /// The CA of the link in PEM.
    pub ca: String,
}

/// A request of the master. Each request travels on its own stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRequest {
    Ping,
}

/// The answer of the agent to one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReply {
    Ok(AgentOutput),
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentOutput {
    Pong { version: String },
}
