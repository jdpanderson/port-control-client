//! One request and its reply over UDP, for PCP and NAT-PMP, and the local
//! address toward a UPnP gateway.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

use tokio::net::UdpSocket;
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
use tokio::time::{Duration, Instant, timeout_at};

#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
use crate::error::Failure;
use crate::error::Result;

/// The PCP and NAT-PMP server port.
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
pub(crate) const PMP_PORT: u16 = 5351;

#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
/// How long NAT-PMP waits after each send. RFC 6886 starts at 250 ms and
/// doubles the wait; we stop after three sends, so a missing server costs
/// 1.75 s and the next protocol gets its turn.
pub(crate) const NAT_PMP_WAITS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
/// PCP messages are at most 1100 bytes (RFC 6887, section 7).
const MAX_MESSAGE: usize = 1100;

/// A UDP socket connected to one server.
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
pub(crate) struct Socket {
    socket: UdpSocket,
    local_ip: Ipv4Addr,
}

#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
impl Socket {
    /// Only datagrams from `server` arrive on the socket.
    pub(crate) async fn connect(server: SocketAddrV4) -> Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        socket.connect(server).await?;
        let local_ip = v4(socket.local_addr()?)?;
        Ok(Self { socket, local_ip })
    }

    /// The local address the system chose for traffic to the server.
    pub(crate) fn local_ip(&self) -> Ipv4Addr {
        self.local_ip
    }

    /// Sends `request` until `parse` accepts a reply. After each send, it
    /// waits for the next of `waits`. `parse` returns `None` for a datagram
    /// that is not a reply to this request.
    pub(crate) async fn exchange<T>(
        &self,
        request: &[u8],
        waits: &[Duration],
        mut parse: impl FnMut(&[u8]) -> Option<Result<T>>,
    ) -> Result<T> {
        let mut buf = [0; MAX_MESSAGE];
        for &wait in waits {
            self.socket.send(request).await?;
            let deadline = Instant::now() + wait;
            // A timeout means: send again.
            while let Ok(received) = timeout_at(deadline, self.socket.recv(&mut buf)).await {
                // An error here is often an ICMP "port unreachable": no
                // server listens.
                if let Some(reply) = parse(&buf[..received?]) {
                    return reply;
                }
            }
        }
        Err(Failure::Timeout)
    }
}

#[cfg(feature = "upnp")]
/// The local address the system would use to send to `peer`. Sends nothing.
pub(crate) async fn local_ip(peer: SocketAddrV4) -> Result<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.connect(peer).await?;
    v4(socket.local_addr()?)
}

fn v4(addr: SocketAddr) -> Result<Ipv4Addr> {
    match addr {
        SocketAddr::V4(a) => Ok(*a.ip()),
        // Not possible: the socket is bound to an IPv4 address.
        SocketAddr::V6(_) => Err(io::Error::other("the socket has an IPv6 address").into()),
    }
}

#[cfg(test)]
mod tests;
