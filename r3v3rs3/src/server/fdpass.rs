//! Passes file descriptors between processes over a Unix stream socket.
//!
//! The stable standard library has no ancillary data API, so this module calls `sendmsg` and
//! `recvmsg` through `libc` and builds one `SCM_RIGHTS` control message itself.

use libc::{
    CMSG_DATA, CMSG_FIRSTHDR, CMSG_LEN, CMSG_NXTHDR, SCM_RIGHTS, SOL_SOCKET, iovec, msghdr,
    recvmsg, sendmsg,
};
use std::io::Result;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;

/// The byte length of the data that one control message with `fds` carries.
fn cmsg_data_len(fds: &[RawFd]) -> usize {
    std::mem::size_of_val(fds)
}

/// Sends the duplicates of the descriptors to the other end of `stream`.
pub fn send_fds(stream: &UnixStream, fds: &[RawFd]) -> Result<()> {
    let data = [1u8];
    let mut io = iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut control = vec![0u8; unsafe { libc::CMSG_SPACE(cmsg_data_len(fds) as _) as usize }];
    // A struct literal would not compile on musl, where msghdr has extra padding fields.
    let mut message: msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut io;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    message.msg_controllen = control.len() as _;
    unsafe {
        let header = message.msg_control as *mut libc::cmsghdr;
        (*header).cmsg_level = SOL_SOCKET;
        (*header).cmsg_type = SCM_RIGHTS;
        (*header).cmsg_len = CMSG_LEN(cmsg_data_len(fds) as _) as _;
        ptr::copy_nonoverlapping(fds.as_ptr(), CMSG_DATA(header) as *mut RawFd, fds.len());
        if sendmsg(stream.as_raw_fd(), &message, 0) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Receives the descriptors from the other end of `stream`, at most `max_fds` of them, and
/// returns however many arrived. The read must start at the beginning of one message, because
/// a read that starts inside an earlier message drops its ancillary data.
pub fn recv_fds(stream: &UnixStream, max_fds: usize) -> Result<Vec<OwnedFd>> {
    let mut data = [0u8; 16];
    let mut io = iovec {
        iov_base: data.as_mut_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut control =
        vec![
            0u8;
            unsafe { libc::CMSG_SPACE((max_fds * std::mem::size_of::<RawFd>()) as _) as usize }
        ];
    // A struct literal would not compile on musl, where msghdr has extra padding fields.
    let mut message: msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut io;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    message.msg_controllen = control.len() as _;
    unsafe {
        if recvmsg(stream.as_raw_fd(), &mut message, 0) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut header = CMSG_FIRSTHDR(&message);
        while let Some(current) = header.as_ref() {
            if current.cmsg_level == SOL_SOCKET && current.cmsg_type == SCM_RIGHTS {
                let data_len = (current.cmsg_len as u64).saturating_sub(CMSG_LEN(0 as _) as u64);
                let received = data_len as usize / std::mem::size_of::<RawFd>();
                let source = CMSG_DATA(current) as *const RawFd;
                return (0..received)
                    .map(|index| {
                        let fd = ptr::read(source.add(index));
                        if fd < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(OwnedFd::from_raw_fd(fd))
                    })
                    .collect();
            }
            header = CMSG_NXTHDR(&message, current);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "the message carried no descriptors",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn a_passed_listener_fd_accepts_a_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, receiver) = UnixStream::pair().unwrap();

        send_fds(&sender, &[listener.as_raw_fd()]).unwrap();
        std::mem::drop(listener);
        let mut passed = recv_fds(&receiver, 1).unwrap();
        assert_eq!(passed.len(), 1);
        let inherited = unsafe {
            TcpListener::from_raw_fd(std::os::fd::IntoRawFd::into_raw_fd(passed.remove(0)))
        };

        let mut client = std::net::TcpStream::connect(addr).unwrap();
        let (mut conn, _) = inherited.accept().unwrap();
        client.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[test]
    fn a_message_that_carries_more_descriptors_than_asked_still_reads_them() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let (sender, receiver) = UnixStream::pair().unwrap();
        send_fds(&sender, &[listener.as_raw_fd()]).unwrap();
        assert_eq!(recv_fds(&receiver, 16).unwrap().len(), 1);
    }

    #[test]
    fn an_end_of_stream_without_descriptors_is_an_error() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        std::mem::drop(sender);
        assert!(recv_fds(&receiver, 1).is_err());
    }
}
