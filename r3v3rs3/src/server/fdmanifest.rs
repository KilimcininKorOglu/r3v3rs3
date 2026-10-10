//! The listening sockets as file descriptors that another process can inherit.

use std::net::SocketAddr;
use std::os::fd::OwnedFd;

/// The protocol that a listening socket serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenerProto {
    Tcp,
    Udp,
    Quic,
}

/// One listening socket of the server: its protocol, its address and a duplicate of the file
/// descriptor that the pool owns. The caller takes the ownership of the duplicate.
#[derive(Debug)]
pub struct ListenerFd {
    pub proto: ListenerProto,
    pub addr: SocketAddr,
    pub fd: OwnedFd,
}

impl ListenerFd {
    pub fn new(proto: ListenerProto, addr: SocketAddr, fd: OwnedFd) -> Self {
        Self { proto, addr, fd }
    }

    /// A duplicate of the descriptor, for a second worker that takes the listener over.
    pub fn try_clone(&self) -> std::io::Result<Self> {
        use std::os::fd::{AsRawFd, BorrowedFd};
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd.as_raw_fd()) }.try_clone_to_owned()?;
        Ok(Self::new(self.proto, self.addr, fd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd};

    #[test]
    fn a_duplicated_listener_fd_accepts_a_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let fd = unsafe { BorrowedFd::borrow_raw(listener.as_raw_fd()) }
            .try_clone_to_owned()
            .unwrap();
        let entry = ListenerFd::new(ListenerProto::Tcp, addr, fd);

        let inherited = unsafe { std::net::TcpListener::from_raw_fd(entry.fd.into_raw_fd()) };
        let mut client = std::net::TcpStream::connect(addr).unwrap();
        let (mut conn, _) = inherited.accept().unwrap();
        client.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
    }
}
