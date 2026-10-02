//! SSDP search for Internet Gateway Devices (UPnP Device Architecture,
//! section 1.3).

use std::{
    io::{self, ErrorKind},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

use tokio::{
    net::UdpSocket,
    time::{Duration, Instant, timeout_at},
};

use super::url::Url;
use crate::error::{Failure, Result};

/// The SSDP multicast group and port.
pub(crate) const MULTICAST: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);

/// Gateways beyond this many are ignored.
const MAX_GATEWAYS: usize = 8;
/// A host can list a gateway for each IGD version, but one host must not
/// take every place.
const MAX_PER_HOST: usize = 2;
/// Replies are usually a few hundred bytes.
const MAX_REPLY: usize = 4096;
/// Windows reports a datagram larger than the buffer with this error.
/// Other systems cut it short, and then it does not parse.
const WSAEMSGSIZE: i32 = 10040;

/// Gateways of both IGD versions. A version 2 gateway should also answer
/// for version 1, but some answer only for their own version.
const TARGETS: [&str; 2] = [
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
];

/// Searches for gateways, and returns their description URLs. Returns as
/// soon as `gateway` answers, with its URL first; otherwise waits `wait`
/// for every answer.
pub(crate) async fn search(
    dest: SocketAddrV4,
    gateway: Option<Ipv4Addr>,
    wait: Duration,
) -> Result<Vec<Url>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    for target in TARGETS {
        socket.send_to(request(target).as_bytes(), dest).await?;
    }
    let deadline = Instant::now() + wait;
    let mut found: Vec<Url> = Vec::new();
    let mut buf = [0; MAX_REPLY];
    while let Ok(received) = timeout_at(deadline, socket.recv_from(&mut buf)).await {
        let (n, from) = match received {
            Ok(received) => received,
            Err(e) if skip(&e) => continue,
            Err(e) => return Err(e.into()),
        };
        let SocketAddr::V4(from) = from else {
            continue;
        };
        let Some(url) = parse_reply(&buf[..n], *from.ip()) else {
            continue;
        };
        // `found` has no URL on this host: its first answer ends here.
        if Some(*url.addr.ip()) == gateway {
            found.insert(0, url);
            return Ok(found);
        }
        keep(&mut found, url);
    }
    if found.is_empty() {
        Err(Failure::NoUpnpGateway)
    } else {
        Ok(found)
    }
}

/// Whether a receive error is about one datagram only, so that the search
/// goes on. Windows reports an ICMP error for an earlier send, and a
/// datagram larger than the buffer, as errors on the socket.
fn skip(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
    ) || (cfg!(windows) && e.raw_os_error() == Some(WSAEMSGSIZE))
}

/// Adds `url` to `found`, unless it is there already or a limit is
/// reached. Each gateway can cost seconds to try. A local network has few.
fn keep(found: &mut Vec<Url>, url: Url) {
    let from_host = found
        .iter()
        .filter(|u| u.addr.ip() == url.addr.ip())
        .count();
    if !found.contains(&url) && found.len() < MAX_GATEWAYS && from_host < MAX_PER_HOST {
        found.push(url);
    }
}

/// Gateways answer within `MX` seconds; `search` waits a little longer.
fn request(target: &str) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 1\r\n\
         ST: {target}\r\n\
         \r\n"
    )
}

/// The description URL in a search reply from `from`. The URL must point
/// back to the host that answered.
fn parse_reply(b: &[u8], from: Ipv4Addr) -> Option<Url> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut reply = httparse::Response::new(&mut headers);
    if !reply.parse(b).ok()?.is_complete() || reply.code != Some(200) {
        return None;
    }
    let location = reply
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))?;
    let url = Url::parse(std::str::from_utf8(location.value).ok()?)?;
    (*url.addr.ip() == from).then_some(url)
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
