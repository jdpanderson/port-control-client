use super::*;

fn sockaddr_in(ip: [u8; 4]) -> Vec<u8> {
    let mut sa = vec![16, libc::AF_INET as u8, 0, 0];
    sa.extend_from_slice(&ip);
    sa.resize(16, 0);
    sa
}

/// A route message with the destination, gateway and netmask. An empty
/// `mask` is a sockaddr of length 0.
fn route(flags: i32, dst: [u8; 4], gateway: [u8; 4], mask: &[u8]) -> Vec<u8> {
    let mut msg = vec![0; HEADER_LEN];
    msg[offset_of!(rt_msghdr, rtm_version)] = libc::RTM_VERSION as u8;
    let addrs = libc::RTA_DST | libc::RTA_GATEWAY | libc::RTA_NETMASK;
    msg[offset_of!(rt_msghdr, rtm_flags)..][..4].copy_from_slice(&flags.to_ne_bytes());
    msg[offset_of!(rt_msghdr, rtm_addrs)..][..4].copy_from_slice(&addrs.to_ne_bytes());
    msg.extend(sockaddr_in(dst));
    msg.extend(sockaddr_in(gateway));
    let padded = if mask.is_empty() {
        4
    } else {
        mask.len().next_multiple_of(4)
    };
    msg.extend_from_slice(mask);
    msg.resize(msg.len() + padded - mask.len(), 0);
    let len = u16::try_from(msg.len()).unwrap();
    msg[..2].copy_from_slice(&len.to_ne_bytes());
    msg
}

const UP: i32 = libc::RTF_UP | libc::RTF_GATEWAY | libc::RTF_STATIC;

#[test]
fn unscoped_default_route_wins() {
    let mut dump = route(UP | libc::RTF_IFSCOPE, [0; 4], [10, 0, 0, 1], &[]);
    dump.extend(route(UP, [0; 4], [192, 168, 1, 1], &[]));
    assert_eq!(parse(&dump).unwrap(), Ipv4Addr::new(192, 168, 1, 1));
    let scoped = route(UP | libc::RTF_IFSCOPE, [0; 4], [10, 0, 0, 1], &[]);
    assert_eq!(parse(&scoped).unwrap(), Ipv4Addr::new(10, 0, 0, 1));
}

#[test]
fn not_default_routes() {
    // 0.0.0.0/1 through a VPN: the mask is 128.0.0.0, cut to 5 bytes.
    let mut dump = route(UP, [0; 4], [10, 0, 0, 1], &[5, 0, 0, 0, 128]);
    // A route to 10.0.0.0/8.
    dump.extend(route(
        UP,
        [10, 0, 0, 0],
        [192, 168, 1, 1],
        &[5, 0, 0, 0, 255],
    ));
    // A default route that is down.
    dump.extend(route(libc::RTF_GATEWAY, [0; 4], [192, 168, 1, 1], &[]));
    assert_eq!(parse(&dump).unwrap_err().kind(), io::ErrorKind::NotFound);
}

#[test]
fn bad_length() {
    let mut dump = route(UP, [0; 4], [192, 168, 1, 1], &[]);
    dump.truncate(dump.len() - 1);
    assert_eq!(parse(&dump).unwrap_err().kind(), io::ErrorKind::InvalidData);
}

/// The parser must read this system's real routing table.
#[test]
fn system_table() {
    let dump = dump().unwrap();
    if let Err(e) = parse(&dump) {
        assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
    }
}

#[test]
fn other_versions_are_skipped() {
    let mut msg = route(UP, [0; 4], [192, 168, 1, 1], &[]);
    msg[offset_of!(rt_msghdr, rtm_version)] = 4;
    assert_eq!(parse(&msg).unwrap_err().kind(), io::ErrorKind::NotFound);
}

#[test]
fn truncated_sockaddrs() {
    // The header names three sockaddrs, but the message ends after the
    // first, or inside the second.
    let full = route(UP, [0; 4], [192, 168, 1, 1], &[]);
    for end in [HEADER_LEN + 16, HEADER_LEN + 20] {
        let mut msg = full[..end].to_vec();
        let len = u16::try_from(msg.len()).unwrap();
        msg[..2].copy_from_slice(&len.to_ne_bytes());
        assert_eq!(parse(&msg).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
