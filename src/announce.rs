//! Restart announcements from PCP and NAT-PMP servers, and the epoch checks
//! that show that a server lost its mappings (RFC 6887, sections 8.5 and
//! 14.1.3; RFC 6886, sections 3.2.1 and 3.6).

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

use socket2::{Domain, Socket, Type};
use tokio::{
    net::UdpSocket,
    time::{Duration, Instant},
};

/// Servers send announcements to this multicast group and port.
const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 1);
const PORT: u16 = 5350;

/// After a PCP announcement, a client waits a random time up to this long
/// before it renews, so that clients do not all send at the same time
/// (RFC 6887, section 14.1.3).
const PCP_DELAY: Duration = Duration::from_secs(5);

/// Where to listen for announcements, and how long to wait before a
/// renewal. Tests use other values.
#[derive(Clone, Debug)]
pub(crate) struct Targets {
    pub(crate) addr: SocketAddrV4,
    /// The multicast group to join there.
    pub(crate) group: Option<Ipv4Addr>,
    /// The longest random wait before a renewal after a PCP restart.
    pub(crate) delay: Duration,
}

impl Targets {
    pub(crate) fn system() -> Self {
        Self {
            addr: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT),
            group: Some(GROUP),
            delay: PCP_DELAY,
        }
    }
}

/// PCP messages are at most 1100 bytes (RFC 6887, section 7).
const MAX_MESSAGE: usize = 1100;

/// The protocol of a server's announcements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Pcp,
    NatPmp,
}

/// A server's epoch time in its last message, and when that message came.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Epoch {
    server: u32,
    at: Instant,
}

impl Epoch {
    pub(crate) fn new(server: u32) -> Self {
        Self {
            server,
            at: Instant::now(),
        }
    }

    /// Whether the epoch time `next`, received at `now`, shows that the
    /// server lost its state since this epoch.
    pub(crate) fn lost(&self, kind: Kind, next: u32, now: Instant) -> bool {
        let (prev, next) = (i64::from(self.server), i64::from(next));
        // Whole seconds on our clock.
        let client = now.saturating_duration_since(self.at).as_secs();
        let client = i64::try_from(client).unwrap_or(i64::MAX);
        match kind {
            // RFC 6887, section 8.5: time goes back by more than a second,
            // or the two clocks move at rates that are too different.
            Kind::Pcp => {
                if next < prev - 1 {
                    return true;
                }
                let server = next - prev;
                client.saturating_add(2) < server - server / 16 || server + 2 < client - client / 16
            }
            // RFC 6886, section 3.6: more than 2 seconds behind 7/8 of the
            // time that passed.
            Kind::NatPmp => next + 2 < prev.saturating_add(client.saturating_mul(7) / 8),
        }
    }
}

/// The epoch time in an announcement of `kind`. `None` for any other
/// datagram.
pub(crate) fn parse(b: &[u8], kind: Kind) -> Option<u32> {
    let epoch = |at: usize| {
        let e = b.get(at..at + 4)?;
        Some(u32::from_be_bytes([e[0], e[1], e[2], e[3]]))
    };
    match kind {
        // A PCP ANNOUNCE response (version 2, the R bit, opcode 0) with
        // result SUCCESS.
        Kind::Pcp if b.len() >= 24 && b[..2] == [2, 0x80] && b[3] == 0 => epoch(8),
        // A NAT-PMP external address response with result 0.
        Kind::NatPmp if b.len() >= 12 && b[..4] == [0, 128, 0, 0] => epoch(4),
        _ => None,
    }
}

/// A socket that receives announcements.
#[derive(Debug)]
pub(crate) struct Listener {
    socket: UdpSocket,
}

impl Listener {
    /// Listens on `addr`. With `group`, it also joins that multicast group
    /// on the interface with the address `interface`.
    pub(crate) fn bind(
        addr: SocketAddrV4,
        group: Option<Ipv4Addr>,
        interface: Ipv4Addr,
    ) -> io::Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(socket2::Protocol::UDP))?;
        // Other clients on this host, such as the system's own, may listen
        // on the same port (RFC 6887, section 14.1.3).
        socket.set_reuse_address(true)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(&SocketAddr::V4(addr).into())?;
        if let Some(group) = group {
            socket.join_multicast_v4(&group, &interface)?;
        }
        let socket = UdpSocket::from_std(socket.into())?;
        Ok(Self { socket })
    }

    /// The epoch time in the next announcement of `kind` from `server`.
    /// Other datagrams are dropped: only the server's own announcements
    /// count.
    pub(crate) async fn next(&self, server: SocketAddrV4, kind: Kind) -> io::Result<u32> {
        let mut buf = [0; MAX_MESSAGE];
        loop {
            let (n, from) = self.socket.recv_from(&mut buf).await?;
            if from == SocketAddr::V4(server) {
                if let Some(epoch) = parse(&buf[..n], kind) {
                    return Ok(epoch);
                }
            }
        }
    }
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
