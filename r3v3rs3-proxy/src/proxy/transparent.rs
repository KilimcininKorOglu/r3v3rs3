//! Socket options for TPROXY range ports and transparent upstreams.
//!
//! The Linux values are written out here, because socket2 offers these options only when it
//! builds for Linux, and the server code has no per-OS branches. On another OS the kernel
//! refuses the unknown option, so a range or transparent port reports an error there.

use socket2::{MaybeUninitSlice, MsgHdrMut, SockAddr, SockRef, Socket};
use std::io;
use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

const SOL_IP: libc::c_int = 0;
const SOL_IPV6: libc::c_int = 41;
const SOL_SOCKET: libc::c_int = 1;
const IP_TRANSPARENT: libc::c_int = 19;
const IP_RECVORIGDSTADDR: libc::c_int = 20;
const IPV6_RECVORIGDSTADDR: libc::c_int = 74;
const IPV6_TRANSPARENT: libc::c_int = 75;
const SO_MARK: libc::c_int = 36;

fn set_int(socket: &Socket, level: libc::c_int, name: libc::c_int, value: u32) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let value = value as libc::c_int;
    // SAFETY: the pointer and the length describe `value`, which lives for the call.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    match result {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

/// Lets the socket take packets for, and send from, an address that is not its own: a TPROXY
/// listener, a reply socket on the original destination, or an upstream socket on the client
/// address. Needs `CAP_NET_ADMIN`.
pub fn set_transparent(socket: &Socket, v6: bool) -> io::Result<()> {
    match v6 {
        true => set_int(socket, SOL_IPV6, IPV6_TRANSPARENT, 1),
        false => set_int(socket, SOL_IP, IP_TRANSPARENT, 1),
    }
}

/// Makes every received datagram carry its original destination.
pub fn set_recv_orig_dst(socket: &Socket, v6: bool) -> io::Result<()> {
    match v6 {
        true => set_int(socket, SOL_IPV6, IPV6_RECVORIGDSTADDR, 1),
        false => set_int(socket, SOL_IP, IP_RECVORIGDSTADDR, 1),
    }
}

/// Sets the firewall mark of the packets the socket sends.
pub fn set_mark(socket: &Socket, mark: u32) -> io::Result<()> {
    set_int(socket, SOL_SOCKET, SO_MARK, mark)
}

/// A UDP socket on `local`, the port of a range that a client sent to, so a reply leaves from
/// that port. It stays unconnected: the measured kernel behaviour is that TPROXY keeps sending
/// the client's datagrams to the range listener, never to this socket. Several sessions share
/// the port, so the address is reusable.
pub fn bind_reply_socket(local: SocketAddr) -> io::Result<tokio::net::UdpSocket> {
    let socket = Socket::new(
        socket2::Domain::for_address(local),
        socket2::Type::DGRAM,
        None,
    )?;
    if local.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    set_transparent(&socket, local.is_ipv6())?;
    socket.set_nonblocking(true)?;
    socket.bind(&local.into())?;
    tokio::net::UdpSocket::from_std(socket.into())
}

/// Receives one datagram with its sender and its original destination, which TPROXY keeps in a
/// control message. The destination is `None` when the message is missing.
pub fn recv_with_orig_dst(
    socket: SockRef<'_>,
    buf: &mut [u8],
) -> io::Result<(usize, SocketAddr, Option<SocketAddr>)> {
    let mut control = [MaybeUninit::<u8>::uninit(); 128];
    // SAFETY: an all-zero sockaddr_storage is a valid, empty address for recvmsg to fill.
    let mut from = unsafe { SockAddr::try_init(|_, _| Ok(())) }?.1;
    // SAFETY: `buf` is initialised memory, so viewing it as possibly uninitialised is sound.
    let slice = unsafe { &mut *(buf as *mut [u8] as *mut [MaybeUninit<u8>]) };
    let mut bufs = [MaybeUninitSlice::new(slice)];
    let mut msg = MsgHdrMut::new()
        .with_addr(&mut from)
        .with_buffers(&mut bufs)
        .with_control(&mut control);
    let size = socket.recvmsg(&mut msg, 0)?;
    let control_len = msg.control_len();
    let sender = from
        .as_socket()
        .ok_or_else(|| io::Error::other("the datagram has no IP sender"))?;
    // SAFETY: recvmsg initialised the first `control_len` bytes of the control buffer.
    let control = unsafe { std::slice::from_raw_parts(control.as_ptr().cast::<u8>(), control_len) };
    Ok((size, sender, orig_dst(control)))
}

/// The original destination in a control buffer of recvmsg.
fn orig_dst(control: &[u8]) -> Option<SocketAddr> {
    let header = std::mem::size_of::<libc::cmsghdr>();
    // SAFETY: CMSG_LEN only computes a size.
    let data_offset = unsafe { libc::CMSG_LEN(0) } as usize;
    let mut offset = 0;
    while offset + header <= control.len() {
        // SAFETY: the header lies inside `control`; the read does not need alignment.
        let cmsg = unsafe {
            std::ptr::read_unaligned(control.as_ptr().add(offset).cast::<libc::cmsghdr>())
        };
        let len = cmsg.cmsg_len as usize;
        if len < data_offset || offset + len > control.len() {
            return None;
        }
        let data = &control[offset + data_offset..offset + len];
        if let Some(addr) = parse_orig_dst(cmsg.cmsg_level, cmsg.cmsg_type, data) {
            return Some(addr);
        }
        // SAFETY: CMSG_SPACE only computes a size.
        offset += unsafe { libc::CMSG_SPACE((len - data_offset) as libc::c_uint) } as usize;
    }
    None
}

/// The address of an `IP_ORIGDSTADDR` (`sockaddr_in`) or `IPV6_ORIGDSTADDR` (`sockaddr_in6`)
/// message. The port and the address are in network order at their Linux offsets.
fn parse_orig_dst(level: libc::c_int, kind: libc::c_int, data: &[u8]) -> Option<SocketAddr> {
    let port = u16::from_be_bytes(data.get(2..4)?.try_into().ok()?);
    let ip = match (level, kind) {
        (SOL_IP, IP_RECVORIGDSTADDR) => {
            IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(data.get(4..8)?).ok()?))
        }
        (SOL_IPV6, IPV6_RECVORIGDSTADDR) => {
            IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(data.get(8..24)?).ok()?))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one control message as the kernel lays it out.
    fn message(level: libc::c_int, kind: libc::c_int, data: &[u8]) -> Vec<u8> {
        // SAFETY: the macros only compute sizes.
        let (space, len) = unsafe {
            (
                libc::CMSG_SPACE(data.len() as libc::c_uint) as usize,
                libc::CMSG_LEN(data.len() as libc::c_uint) as usize,
            )
        };
        let mut buf = vec![0u8; space];
        // SAFETY: an all-zero cmsghdr is valid; its fields are set below.
        let mut header: libc::cmsghdr = unsafe { std::mem::zeroed() };
        header.cmsg_len = len as _;
        header.cmsg_level = level;
        header.cmsg_type = kind;
        // SAFETY: `buf` holds at least one header.
        unsafe { std::ptr::write_unaligned(buf.as_mut_ptr().cast(), header) };
        // SAFETY: CMSG_LEN only computes a size.
        let offset = unsafe { libc::CMSG_LEN(0) } as usize;
        buf[offset..offset + data.len()].copy_from_slice(data);
        buf
    }

    /// The original destination comes from the `IP_ORIGDSTADDR` message (Linux ip(7)): a
    /// `sockaddr_in` with the port and the address in network order.
    #[test]
    fn the_original_destination_is_read_from_its_control_message() {
        let mut sockaddr_in = vec![2, 0];
        sockaddr_in.extend(40005u16.to_be_bytes());
        sockaddr_in.extend([10, 9, 0, 1]);
        sockaddr_in.extend([0; 8]);
        let control = message(SOL_IP, IP_RECVORIGDSTADDR, &sockaddr_in);
        assert_eq!(orig_dst(&control), Some("10.9.0.1:40005".parse().unwrap()));

        let mut sockaddr_in6 = vec![10, 0];
        sockaddr_in6.extend(49160u16.to_be_bytes());
        sockaddr_in6.extend([0; 4]);
        sockaddr_in6.extend("2001:db8::5".parse::<Ipv6Addr>().unwrap().octets());
        sockaddr_in6.extend([0; 4]);
        let control6 = message(SOL_IPV6, IPV6_RECVORIGDSTADDR, &sockaddr_in6);
        assert_eq!(
            orig_dst(&control6),
            Some("[2001:db8::5]:49160".parse().unwrap())
        );

        // Another message before it is skipped.
        let mut both = message(SOL_SOCKET, 29, &[0; 16]);
        both.extend(&control);
        assert_eq!(orig_dst(&both), Some("10.9.0.1:40005".parse().unwrap()));
    }

    /// A missing, foreign or cut message gives no destination instead of a wrong one.
    #[test]
    fn no_or_broken_control_message_gives_no_destination() {
        assert_eq!(orig_dst(&[]), None);
        assert_eq!(orig_dst(&message(SOL_SOCKET, 29, &[0; 16])), None);
        let mut cut = message(SOL_IP, IP_RECVORIGDSTADDR, &[2, 0, 0, 1, 10, 0, 0, 1]);
        cut.truncate(cut.len() - 4);
        assert_eq!(orig_dst(&cut), None);
        assert_eq!(parse_orig_dst(SOL_IP, IP_RECVORIGDSTADDR, &[2, 0]), None);
    }
}
