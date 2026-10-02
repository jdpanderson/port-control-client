//! For the fuzz target: any datagram as a mapping or an external address
//! response.

use super::{be16, parse_external_address, parse_map_response};
use crate::Protocol;

pub(crate) fn run(data: &[u8]) {
    let port = data.get(8..10).map_or(0, be16);
    for protocol in [Protocol::Udp, Protocol::Tcp] {
        let _ = parse_map_response(data, protocol, port);
    }
    let _ = parse_external_address(data);
}
