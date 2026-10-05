//! PCP, the Port Control Protocol (RFC 6887): the MAP opcode, for IPv4.

use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddrV4},
    time::Duration,
};

#[cfg(feature = "restart-announcements")]
use crate::announce::Epoch;
use crate::{
    Protocol,
    error::{Failure, Result},
    random_fraction,
    udp::{NAT_PMP_WAITS, Socket},
};

/// How long to wait for a reply after a send: the initial retransmission
/// time of RFC 6887, section 8.1.1.
pub(crate) const WAIT: Duration = Duration::from_secs(3);

const VERSION: u8 = 2;
const MAP: u8 = 1;
/// The R bit in the opcode byte marks a response.
const RESPONSE: u8 = 0x80;
const HEADER_LEN: usize = 24;
/// The common header and the MAP opcode data.
const MAP_LEN: usize = HEADER_LEN + 36;

pub(crate) type Nonce = [u8; 12];

/// A mapping granted by a PCP server.
#[derive(Clone, Debug)]
pub(crate) struct Lease {
    pub(crate) server: SocketAddrV4,
    pub(crate) local_ip: Ipv4Addr,
    /// Identifies the mapping to the server, for renewal and release.
    pub(crate) nonce: Nonce,
    pub(crate) protocol: Protocol,
    pub(crate) local_port: u16,
    pub(crate) external: SocketAddrV4,
    pub(crate) lifetime: Duration,
    /// The server's epoch time in the last reply, for restart
    /// announcements.
    #[cfg(feature = "restart-announcements")]
    pub(crate) epoch: Epoch,
}

/// Asks `server` for a mapping of `local_port`. Use the same `nonce` for
/// every request: a server refuses a new nonce for a mapping it still has
/// (RFC 6887, section 11.3), and with the same nonce, a request after a
/// lost reply renews the mapping that the server made. `wait` is the time
/// to wait for a reply; see [`WAIT`].
pub(crate) async fn map(
    server: SocketAddrV4,
    nonce: Nonce,
    protocol: Protocol,
    local_port: u16,
    lifetime: u32,
    wait: Duration,
) -> Result<Lease> {
    let socket = Socket::connect(server).await?;
    let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let waits = waits(wait);
    let granted = send(&socket, &nonce, protocol, local_port, any, lifetime, &waits).await?;
    granted.lease(socket.local_ip(), server, nonce, protocol, local_port)
}

/// Renews `lease`, and asks to keep its external address and port.
pub(crate) async fn renew(lease: &Lease, lifetime: u32, wait: Duration) -> Result<Lease> {
    let socket = Socket::connect(lease.server).await?;
    if socket.local_ip() != lease.local_ip {
        return Err(Failure::NetworkChanged);
    }
    let granted = send(
        &socket,
        &lease.nonce,
        lease.protocol,
        lease.local_port,
        lease.external,
        lifetime,
        &waits(wait),
    )
    .await?;
    granted.lease(
        socket.local_ip(),
        lease.server,
        lease.nonce,
        lease.protocol,
        lease.local_port,
    )
}

/// Deletes `lease` on the server: a MAP request with lifetime 0. It uses
/// the short NAT-PMP waits, because `stop` waits only two seconds.
pub(crate) async fn release(lease: &Lease) -> Result<()> {
    let socket = Socket::connect(lease.server).await?;
    send(
        &socket,
        &lease.nonce,
        lease.protocol,
        lease.local_port,
        lease.external,
        0,
        &NAT_PMP_WAITS,
    )
    .await?;
    Ok(())
}

/// RFC 6887 (section 8.1.1) doubles the wait after each send, with no
/// limit. We send twice, then let the next protocol try. Each wait changes
/// by a random -10% to +10%, as the RFC says, so that clients do not send
/// at the same time.
fn waits(wait: Duration) -> [Duration; 2] {
    let jitter = || wait.mul_f64(0.9 + 0.2 * random_fraction());
    [jitter(), jitter()]
}

/// What the server granted.
#[derive(Debug, PartialEq, Eq)]
struct Granted {
    lifetime: u32,
    external: SocketAddrV4,
    epoch: u32,
}

impl Granted {
    fn lease(
        self,
        local_ip: Ipv4Addr,
        server: SocketAddrV4,
        nonce: Nonce,
        protocol: Protocol,
        local_port: u16,
    ) -> Result<Lease> {
        if self.lifetime == 0 {
            return Err(Failure::BadReply("granted lifetime is 0"));
        }
        if self.external.ip().is_unspecified() || self.external.port() == 0 {
            return Err(Failure::BadReply("no external address"));
        }
        Ok(Lease {
            server,
            local_ip,
            nonce,
            protocol,
            local_port,
            external: self.external,
            lifetime: Duration::from_secs(self.lifetime.into()),
            #[cfg(feature = "restart-announcements")]
            epoch: Epoch::new(self.epoch),
        })
    }
}

async fn send(
    socket: &Socket,
    nonce: &Nonce,
    protocol: Protocol,
    local_port: u16,
    suggested: SocketAddrV4,
    lifetime: u32,
    waits: &[Duration],
) -> Result<Granted> {
    let client = socket.local_ip();
    let request = map_request(nonce, protocol, client, local_port, suggested, lifetime);
    socket
        .exchange(&request, waits, |reply| {
            parse_map_response(reply, nonce, protocol, local_port)
        })
        .await
}

fn map_request(
    nonce: &Nonce,
    protocol: Protocol,
    client: Ipv4Addr,
    local_port: u16,
    suggested: SocketAddrV4,
    lifetime: u32,
) -> [u8; MAP_LEN] {
    let mut b = [0; MAP_LEN];
    // Header: version, opcode, 2 reserved bytes, lifetime, client address.
    b[0] = VERSION;
    b[1] = MAP;
    b[4..8].copy_from_slice(&lifetime.to_be_bytes());
    b[8..24].copy_from_slice(&client.to_ipv6_mapped().octets());
    // MAP data: nonce, protocol, 3 reserved bytes, internal port, suggested
    // external port and address. The address ::ffff:0.0.0.0 asks for any
    // IPv4 address.
    b[24..36].copy_from_slice(nonce);
    b[36] = protocol_number(protocol);
    b[40..42].copy_from_slice(&local_port.to_be_bytes());
    // In a delete, the suggested address and port must be zero (RFC 6887,
    // section 15).
    if lifetime != 0 {
        b[42..44].copy_from_slice(&suggested.port().to_be_bytes());
        b[44..60].copy_from_slice(&suggested.ip().to_ipv6_mapped().octets());
    }
    b
}

/// `None` for a datagram that is not a reply to this request.
fn parse_map_response(
    b: &[u8],
    nonce: &Nonce,
    protocol: Protocol,
    local_port: u16,
) -> Option<Result<Granted>> {
    if b.is_empty() {
        return None;
    }
    // A NAT-PMP server answers in its own version (RFC 6887, appendix A).
    if b[0] != VERSION {
        return Some(Err(refused(1)));
    }
    if b.len() < HEADER_LEN || b[1] != RESPONSE | MAP {
        return None;
    }
    // A server should copy the MAP data into an error response, but it may
    // send only the header.
    if b.len() >= MAP_LEN && b[24..36] != nonce[..] {
        return None;
    }
    if b[3] != 0 {
        return Some(Err(refused(b[3])));
    }
    if b.len() < MAP_LEN {
        return Some(Err(Failure::BadReply("short MAP response")));
    }
    if b[36] != protocol_number(protocol) || be16(&b[40..42]) != local_port {
        return None;
    }
    let lifetime = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    let epoch = u32::from_be_bytes([b[8], b[9], b[10], b[11]]);
    let mut ip = [0; 16];
    ip.copy_from_slice(&b[44..60]);
    let ip = Ipv6Addr::from(ip);
    // A delete's reply holds the zero address of its request.
    let ip = if ip.is_unspecified() {
        Some(Ipv4Addr::UNSPECIFIED)
    } else {
        ip.to_ipv4_mapped()
    };
    let Some(ip) = ip else {
        return Some(Err(Failure::BadReply("external address is not IPv4")));
    };
    let external = SocketAddrV4::new(ip, be16(&b[42..44]));
    Some(Ok(Granted {
        lifetime,
        external,
        epoch,
    }))
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn protocol_number(protocol: Protocol) -> u8 {
    match protocol {
        Protocol::Udp => 17,
        Protocol::Tcp => 6,
    }
}

fn refused(code: u8) -> Failure {
    let name = match code {
        1 => "UNSUPP_VERSION",
        2 => "NOT_AUTHORIZED",
        3 => "MALFORMED_REQUEST",
        4 => "UNSUPP_OPCODE",
        5 => "UNSUPP_OPTION",
        6 => "MALFORMED_OPTION",
        7 => "NETWORK_FAILURE",
        8 => "NO_RESOURCES",
        9 => "UNSUPP_PROTOCOL",
        10 => "USER_EX_QUOTA",
        11 => "CANNOT_PROVIDE_EXTERNAL",
        12 => "ADDRESS_MISMATCH",
        13 => "EXCESSIVE_REMOTE_PEERS",
        _ => "unknown",
    };
    Failure::Refused {
        code: code.into(),
        name,
        // The "short lifetime" errors of RFC 6887, section 7.4.
        temporary: matches!(code, 7 | 8 | 10 | 11),
    }
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
