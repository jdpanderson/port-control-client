//! For the fuzz target: any datagram as a MAP response. The nonce,
//! protocol and port come from where a response has them, so that the
//! fuzzer can reach a success.

use std::net::{Ipv4Addr, SocketAddrV4};

use super::{MAP_LEN, be16, parse_map_response};
use crate::{Protocol, udp::PMP_PORT as PORT};

pub(crate) fn run(data: &[u8]) {
    let mut nonce = [0; 12];
    if let Some(n) = data.get(24..36) {
        nonce.copy_from_slice(n);
    }
    let port = data.get(40..42).map_or(0, be16);
    for protocol in [Protocol::Udp, Protocol::Tcp] {
        if let Some(Ok(granted)) = parse_map_response(data, &nonce, protocol, port) {
            assert!(data.len() >= MAP_LEN, "a success has the MAP data");
            let server = SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT);
            let _ = granted.lease(Ipv4Addr::LOCALHOST, server, nonce, protocol, port);
        }
    }
}
