use super::*;

const NONCE: Nonce = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
const EPOCH: u32 = 1000;

fn response(result: u8, lifetime: u32, nonce: &Nonce, external: SocketAddrV4) -> Vec<u8> {
    let mut b = vec![0; MAP_LEN];
    b[0] = VERSION;
    b[1] = RESPONSE | MAP;
    b[3] = result;
    b[4..8].copy_from_slice(&lifetime.to_be_bytes());
    b[8..12].copy_from_slice(&EPOCH.to_be_bytes());
    b[24..36].copy_from_slice(nonce);
    b[36] = 17;
    b[40..42].copy_from_slice(&51820u16.to_be_bytes());
    b[42..44].copy_from_slice(&external.port().to_be_bytes());
    b[44..60].copy_from_slice(&external.ip().to_ipv6_mapped().octets());
    b
}

#[test]
fn request_layout() {
    let client = Ipv4Addr::new(192, 168, 1, 10);
    let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let b = map_request(&NONCE, Protocol::Udp, client, 51820, any, 7200);
    let mut want = [0u8; 60];
    want[0] = 2;
    want[1] = 1;
    want[4..8].copy_from_slice(&[0, 0, 0x1c, 0x20]);
    want[18..24].copy_from_slice(&[0xff, 0xff, 192, 168, 1, 10]);
    want[24..36].copy_from_slice(&NONCE);
    want[36] = 17;
    want[40..42].copy_from_slice(&[0xca, 0x6c]);
    want[54..56].copy_from_slice(&[0xff, 0xff]);
    assert_eq!(b, want);
}

#[test]
fn delete_has_no_suggestion() {
    let client = Ipv4Addr::new(192, 168, 1, 10);
    let old = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 40000);
    let b = map_request(&NONCE, Protocol::Udp, client, 51820, old, 0);
    assert_eq!(b[4..8], [0; 4]);
    assert_eq!(b[42..60], [0; 18]);
    // The reply copies the zeros.
    let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let mut reply = response(0, 0, &NONCE, any);
    reply[44..60].fill(0);
    let got = parse_map_response(&reply, &NONCE, Protocol::Udp, 51820);
    assert_eq!(
        got.unwrap().unwrap(),
        Granted {
            lifetime: 0,
            external: any,
            epoch: EPOCH
        }
    );
}

#[test]
fn short_lifetime_errors() {
    for (code, want) in [(8, true), (2, false)] {
        let got = refused(code);
        let ok = matches!(got, Failure::Refused { temporary, .. } if temporary == want);
        assert!(ok, "{code}: {got:?}");
    }
}

#[test]
fn success() {
    let external = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 40000);
    let b = response(0, 3600, &NONCE, external);
    let got = parse_map_response(&b, &NONCE, Protocol::Udp, 51820);
    assert_eq!(
        got.unwrap().unwrap(),
        Granted {
            lifetime: 3600,
            external,
            epoch: EPOCH
        }
    );
}

#[test]
fn ignores_replies_to_other_requests() {
    let external = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 40000);
    let b = response(0, 3600, &NONCE, external);
    // Another nonce, protocol or port.
    for (nonce, protocol, port) in [
        ([9; 12], Protocol::Udp, 51820),
        (NONCE, Protocol::Tcp, 51820),
        (NONCE, Protocol::Udp, 1),
    ] {
        let got = parse_map_response(&b, &nonce, protocol, port);
        assert!(got.is_none(), "{nonce:?} {protocol} {port}");
    }
}

#[test]
fn errors() {
    let external = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let b = response(2, 0, &NONCE, external);
    let err = parse_map_response(&b, &NONCE, Protocol::Udp, 51820).unwrap();
    assert!(matches!(err, Err(Failure::Refused { code: 2, .. })));
    // Only the header.
    let err = parse_map_response(&b[..HEADER_LEN], &NONCE, Protocol::Udp, 51820).unwrap();
    assert!(matches!(err, Err(Failure::Refused { code: 2, .. })));
}

#[test]
fn nat_pmp_server() {
    // A NAT-PMP server's "unsupported version" reply.
    let b = [0, 0x81, 0, 1, 0, 0, 0, 0];
    let err = parse_map_response(&b, &NONCE, Protocol::Udp, 51820).unwrap();
    assert!(matches!(err, Err(Failure::Refused { code: 1, .. })));
}

#[test]
fn not_replies() {
    let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let b = response(0, 3600, &NONCE, any);
    // A reply to another opcode (PEER).
    let mut peer = b.clone();
    peer[1] = RESPONSE | 2;
    // Empty, or too short for a header.
    for b in [&[][..], &b[..10], &peer] {
        let got = parse_map_response(b, &NONCE, Protocol::Udp, 51820);
        assert!(got.is_none(), "{b:?}");
    }
}

#[test]
fn bad_replies() {
    let external = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 40000);
    let b = response(0, 3600, &NONCE, external);
    // An IPv6 external address.
    let mut ipv6 = b.clone();
    let addr: Ipv6Addr = "2001:db8::1".parse().unwrap();
    ipv6[44..60].copy_from_slice(&addr.octets());
    // A success with no MAP data.
    for b in [&b[..HEADER_LEN], &ipv6] {
        let got = parse_map_response(b, &NONCE, Protocol::Udp, 51820);
        assert!(matches!(got, Some(Err(Failure::BadReply(_)))), "{b:?}");
    }
}

#[test]
fn grants_that_are_not_mappings() {
    let server = SocketAddrV4::new(Ipv4Addr::LOCALHOST, crate::udp::PMP_PORT);
    let lease = |lifetime, external| {
        let granted = Granted {
            lifetime,
            external,
            epoch: EPOCH,
        };
        granted.lease(Ipv4Addr::LOCALHOST, server, NONCE, Protocol::Udp, 51820)
    };
    let ip = Ipv4Addr::new(203, 0, 113, 5);
    assert!(lease(3600, SocketAddrV4::new(ip, 40000)).is_ok());
    // No lifetime, no address, no port, or neither.
    for (lifetime, ip, port) in [
        (0, ip, 40000),
        (3600, Ipv4Addr::UNSPECIFIED, 40000),
        (3600, ip, 0),
        (3600, Ipv4Addr::UNSPECIFIED, 0),
    ] {
        let got = lease(lifetime, SocketAddrV4::new(ip, port));
        assert!(
            matches!(got, Err(Failure::BadReply(_))),
            "{lifetime} {ip}:{port}"
        );
    }
}

#[tokio::test]
async fn renewal_from_another_address() {
    let lease = Lease {
        server: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9),
        local_ip: Ipv4Addr::new(10, 9, 9, 9),
        nonce: NONCE,
        protocol: Protocol::Udp,
        local_port: 51820,
        external: SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 40000),
        lifetime: Duration::from_secs(60),
        #[cfg(feature = "restart-announcements")]
        epoch: Epoch::new(EPOCH),
    };
    assert!(matches!(
        renew(&lease, 60, WAIT).await,
        Err(Failure::NetworkChanged)
    ));
}

#[test]
fn waits_vary_by_a_tenth() {
    for _ in 0..100 {
        for wait in waits(WAIT) {
            assert!(
                wait >= WAIT.mul_f64(0.9) && wait <= WAIT.mul_f64(1.1),
                "{wait:?}"
            );
        }
    }
}

#[test]
fn result_names() {
    for code in 1..=13 {
        let known = matches!(refused(code), Failure::Refused { name, .. } if name != "unknown");
        assert!(known, "{code}");
    }
    assert!(matches!(
        refused(14),
        Failure::Refused {
            name: "unknown",
            ..
        }
    ));
}
