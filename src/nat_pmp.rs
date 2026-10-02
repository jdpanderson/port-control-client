//! NAT-PMP (RFC 6886).

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

#[cfg(feature = "restart-announcements")]
use crate::announce::Epoch;
use crate::{
    Protocol,
    error::{Failure, Result},
    udp::{NAT_PMP_WAITS, Socket},
};

const VERSION: u8 = 0;
const EXTERNAL_ADDRESS: u8 = 0;
/// A response's opcode is the request's plus 128.
const RESPONSE: u8 = 128;

/// A mapping granted by a NAT-PMP server.
#[derive(Clone, Debug)]
pub(crate) struct Lease {
    pub(crate) server: SocketAddrV4,
    pub(crate) local_ip: Ipv4Addr,
    pub(crate) protocol: Protocol,
    pub(crate) local_port: u16,
    pub(crate) external: SocketAddrV4,
    pub(crate) lifetime: Duration,
    /// The server's epoch time in the last reply, for restart
    /// announcements.
    #[cfg(feature = "restart-announcements")]
    pub(crate) epoch: Epoch,
}

/// Asks `server` for a new mapping of `local_port`.
pub(crate) async fn map(
    server: SocketAddrV4,
    protocol: Protocol,
    local_port: u16,
    lifetime: u32,
) -> Result<Lease> {
    let socket = Socket::connect(server).await?;
    // RFC 6886 suggests the internal port as the external port.
    lease(&socket, server, protocol, local_port, local_port, lifetime).await
}

/// Renews `lease`, and asks to keep its external port.
pub(crate) async fn renew(lease: &Lease, lifetime: u32) -> Result<Lease> {
    let socket = Socket::connect(lease.server).await?;
    if socket.local_ip() != lease.local_ip {
        return Err(Failure::NetworkChanged);
    }
    let suggested = lease.external.port();
    self::lease(
        &socket,
        lease.server,
        lease.protocol,
        lease.local_port,
        suggested,
        lifetime,
    )
    .await
}

/// Deletes `lease` on the server: lifetime 0 and external port 0.
pub(crate) async fn release(lease: &Lease) -> Result<()> {
    let socket = Socket::connect(lease.server).await?;
    let request = map_request(lease.protocol, lease.local_port, 0, 0);
    socket
        .exchange(&request, &NAT_PMP_WAITS, |reply| {
            parse_map_response(reply, lease.protocol, lease.local_port)
        })
        .await?;
    Ok(())
}

async fn lease(
    socket: &Socket,
    server: SocketAddrV4,
    protocol: Protocol,
    local_port: u16,
    suggested: u16,
    lifetime: u32,
) -> Result<Lease> {
    // The external address comes first: if it fails, there is no mapping
    // to delete.
    let ip = socket
        .exchange(
            &[VERSION, EXTERNAL_ADDRESS],
            &NAT_PMP_WAITS,
            parse_external_address,
        )
        .await?;
    if ip.is_unspecified() {
        return Err(Failure::BadReply("the router has no external address"));
    }
    let request = map_request(protocol, local_port, suggested, lifetime);
    let (external_port, lifetime, epoch) = socket
        .exchange(&request, &NAT_PMP_WAITS, |reply| {
            parse_map_response(reply, protocol, local_port)
        })
        .await?;
    if lifetime == 0 || external_port == 0 {
        return Err(Failure::BadReply("no mapping granted"));
    }
    // Only restart announcements use the epoch time.
    #[cfg(not(feature = "restart-announcements"))]
    let _ = epoch;
    Ok(Lease {
        server,
        local_ip: socket.local_ip(),
        protocol,
        local_port,
        external: SocketAddrV4::new(ip, external_port),
        lifetime: Duration::from_secs(lifetime.into()),
        #[cfg(feature = "restart-announcements")]
        epoch: Epoch::new(epoch),
    })
}

fn map_request(protocol: Protocol, local_port: u16, suggested: u16, lifetime: u32) -> [u8; 12] {
    // Version, opcode, 2 reserved bytes, internal port, suggested external
    // port, lifetime.
    let mut b = [0; 12];
    b[0] = VERSION;
    b[1] = opcode(protocol);
    b[4..6].copy_from_slice(&local_port.to_be_bytes());
    b[6..8].copy_from_slice(&suggested.to_be_bytes());
    b[8..12].copy_from_slice(&lifetime.to_be_bytes());
    b
}

/// The external port, the lifetime and the server's epoch time. `None`
/// for a datagram that is not a reply to this request.
fn parse_map_response(
    b: &[u8],
    protocol: Protocol,
    local_port: u16,
) -> Option<Result<(u16, u32, u32)>> {
    if let Err(e) = check_header(b, opcode(protocol))? {
        return Some(Err(e));
    }
    if b.len() < 16 {
        return Some(Err(Failure::BadReply("short NAT-PMP response")));
    }
    if be16(&b[8..10]) != local_port {
        return None;
    }
    let epoch = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    let lifetime = u32::from_be_bytes([b[12], b[13], b[14], b[15]]);
    Some(Ok((be16(&b[10..12]), lifetime, epoch)))
}

fn parse_external_address(b: &[u8]) -> Option<Result<Ipv4Addr>> {
    if let Err(e) = check_header(b, EXTERNAL_ADDRESS)? {
        return Some(Err(e));
    }
    if b.len() < 12 {
        return Some(Err(Failure::BadReply("short NAT-PMP response")));
    }
    Some(Ok(Ipv4Addr::new(b[8], b[9], b[10], b[11])))
}

/// Checks the version, opcode and result code. `None` for a response to
/// another opcode.
fn check_header(b: &[u8], op: u8) -> Option<Result<()>> {
    if b.is_empty() {
        return None;
    }
    if b[0] != VERSION {
        return Some(Err(Failure::BadReply("not a NAT-PMP response")));
    }
    if b.len() < 4 || b[1] != RESPONSE + op {
        return None;
    }
    match be16(&b[2..4]) {
        0 => Some(Ok(())),
        code => Some(Err(refused(code))),
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn opcode(protocol: Protocol) -> u8 {
    match protocol {
        Protocol::Udp => 1,
        Protocol::Tcp => 2,
    }
}

fn refused(code: u16) -> Failure {
    let name = match code {
        1 => "unsupported version",
        2 => "not authorized",
        3 => "network failure",
        4 => "out of resources",
        5 => "unsupported opcode",
        _ => "unknown",
    };
    Failure::Refused {
        code,
        name,
        // Network failure and out of resources (RFC 6886, section 3.5) can
        // pass with time.
        temporary: matches!(code, 3 | 4),
    }
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
