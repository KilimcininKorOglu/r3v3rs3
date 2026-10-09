//! The control channel between the master process and a worker process.
//!
//! A frame is a little-endian `u32` byte length followed by that many bytes of JSON. The
//! listening sockets travel out of band as file descriptors, so a frame carries only their
//! addresses.

use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::ProxyEntry;
use serde_derive::{Deserialize, Serialize};
use std::io::{Read, Result, Write};
use std::net::SocketAddr;

/// The version of the frame layout and of the messages. A peer that reads another version
/// refuses the channel, so an old master and a new worker never mix their state.
pub const PROTOCOL_VERSION: u32 = 1;

const MAX_FRAME_SIZE: u32 = 64 * 1024 * 1024;

/// The metadata of one listening socket that travels with its file descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenerMeta {
    pub proto: crate::server::fdmanifest::ListenerProto,
    pub addr: SocketAddr,
}

/// The first frame of the channel. The descriptors of `listeners` arrive over the same socket
/// as ancillary data right after this frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    pub protocol: u32,
    pub listeners: Vec<ListenerMeta>,
}

/// What the master tells a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerCommand {
    /// Replaces the configuration that the worker serves.
    Config(Box<WorkerConfig>),
    /// Stops accepting and waits for the open connections.
    Drain,
    /// Drops the connections and exits.
    Shutdown,
    /// The worker answers with [`WorkerEvent::Pong`].
    Ping,
}

/// The configuration that a worker needs. It reads nothing from the storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub ports: Vec<PortEntry>,
    pub proxies: Vec<ProxyEntry>,
    pub app: AppConfig,
}

/// What a worker tells the master.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerEvent {
    Pong,
    /// The worker gave up. The master restarts it.
    Fatal(String),
    /// A server event of the worker, for example a status update.
    Event(ServerEvent),
}

/// Writes one JSON frame.
pub fn write_frame<T: serde::ser::Serialize>(stream: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let len = u32::try_from(bytes.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "the frame is too long")
    })?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&bytes)
}

/// Reads one JSON frame.
pub fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut impl Read) -> Result<T> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the frame length exceeds the limit",
        ));
    }
    let mut bytes = vec![0u8; len as usize];
    stream.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_command_round_trips_through_a_socket() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        let command = WorkerCommand::Config(Box::new(WorkerConfig {
            ports: Vec::new(),
            proxies: Vec::new(),
            app: AppConfig::default(),
        }));
        write_frame(&mut sender, &command).unwrap();
        let received: WorkerCommand = read_frame(&mut receiver).unwrap();
        assert_eq!(received, command);
    }

    #[test]
    fn a_truncated_frame_is_an_error() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        let bytes = serde_json::to_vec(&WorkerCommand::Ping).unwrap();
        sender
            .write_all(&(bytes.len() as u32 + 1).to_le_bytes())
            .unwrap();
        sender.write_all(&bytes).unwrap();
        // The reader blocks on the missing byte until the sender hangs up.
        std::mem::drop(sender);
        let result: Result<WorkerCommand> = read_frame(&mut receiver);
        assert!(result.is_err());
    }

    #[test]
    fn an_overlong_frame_length_is_an_error() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&u32::MAX.to_le_bytes()).unwrap();
        let result: Result<WorkerCommand> = read_frame(&mut receiver);
        assert!(result.is_err());
    }
}
