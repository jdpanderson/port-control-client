//! For the fuzz target: any datagram as a search reply. A URL must point
//! back to the host that answered.

use std::net::Ipv4Addr;

use super::parse_reply;

pub(crate) fn run(data: &[u8]) {
    let from = Ipv4Addr::new(192, 168, 1, 1);
    if let Some(url) = parse_reply(data, from) {
        assert_eq!(*url.addr.ip(), from);
    }
}
