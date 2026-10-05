use super::*;
use tokio::task::JoinHandle;

use crate::responder::responder;

#[test]
fn request_layout() {
    let b = map_request(Protocol::Tcp, 51820, 40000, 7200);
    assert_eq!(b, [0, 2, 0, 0, 0xca, 0x6c, 0x9c, 0x40, 0, 0, 0x1c, 0x20]);
}

#[test]
fn map_response() {
    let b = [
        0, 129, 0, 0, 0, 0, 0, 9, 0xca, 0x6c, 0x9c, 0x40, 0, 0, 0x0e, 0x10,
    ];
    let got = parse_map_response(&b, Protocol::Udp, 51820)
        .unwrap()
        .unwrap();
    assert_eq!(got, (40000, 3600, 9));
    // For another port, or another protocol.
    assert!(parse_map_response(&b, Protocol::Udp, 1).is_none());
    assert!(parse_map_response(&b, Protocol::Tcp, 51820).is_none());
}

#[test]
fn external_address() {
    let b = [0, 128, 0, 0, 0, 0, 0, 9, 203, 0, 113, 5];
    let got = parse_external_address(&b).unwrap().unwrap();
    assert_eq!(got, Ipv4Addr::new(203, 0, 113, 5));
}

#[test]
fn errors() {
    let b = [0, 129, 0, 2, 0, 0, 0, 9, 0xca, 0x6c, 0, 0, 0, 0, 0, 0];
    let err = parse_map_response(&b, Protocol::Udp, 51820).unwrap();
    assert!(matches!(err, Err(Failure::Refused { code: 2, .. })));
    // A PCP server's reply.
    let err = parse_external_address(&[2, 0x80, 0, 1]).unwrap();
    assert!(matches!(err, Err(Failure::BadReply(_))));
}

#[test]
fn not_replies() {
    // Empty, or too short for a header.
    for b in [&[][..], &[0, 129]] {
        assert!(
            parse_map_response(b, Protocol::Udp, 51820).is_none(),
            "{b:?}"
        );
    }
    // A reply to another opcode.
    assert!(parse_external_address(&[0, 129, 0, 0]).is_none());
}

#[test]
fn short_replies() {
    let got = parse_map_response(&[0, 129, 0, 0, 0, 0, 0, 9], Protocol::Udp, 51820);
    assert!(matches!(got, Some(Err(Failure::BadReply(_)))));
    // No address, or only the 4-byte header of a success.
    for b in [&[0, 128, 0, 0, 0, 0, 0, 9][..], &[0, 128, 0, 0]] {
        let got = parse_external_address(b);
        assert!(matches!(got, Some(Err(Failure::BadReply(_)))), "{b:?}");
    }
    // Only the header of a refusal.
    let got = parse_external_address(&[0, 128, 0, 3]);
    assert!(matches!(got, Some(Err(Failure::Refused { code: 3, .. }))));
}

/// A server that grants `lifetime`, with the internal port as the
/// external port, and reports `external`.
async fn server(lifetime: u32, external: Ipv4Addr) -> (SocketAddrV4, JoinHandle<()>) {
    responder(move |request| {
        let mut reply = vec![0, RESPONSE + request[1], 0, 0, 0, 0, 0, 1];
        if request[1] == EXTERNAL_ADDRESS {
            reply.extend_from_slice(&external.octets());
        } else {
            reply.extend_from_slice(&request[4..6]);
            reply.extend_from_slice(&request[4..6]);
            reply.extend_from_slice(&lifetime.to_be_bytes());
        }
        vec![reply]
    })
    .await
}

#[tokio::test]
async fn grants_that_are_not_mappings() {
    let ip = Ipv4Addr::new(203, 0, 113, 5);
    let (addr, _task) = server(3600, ip).await;
    let lease = map(addr, Protocol::Udp, 51820, 3600).await.unwrap();
    assert_eq!(lease.external, SocketAddrV4::new(ip, 51820));
    // No lifetime, or no external address.
    for (lifetime, ip) in [(0, ip), (3600, Ipv4Addr::UNSPECIFIED)] {
        let (addr, _task) = server(lifetime, ip).await;
        let got = map(addr, Protocol::Udp, 51820, 3600).await;
        assert!(matches!(got, Err(Failure::BadReply(_))), "{lifetime} {ip}");
    }
}

#[tokio::test]
async fn renewal_from_another_address() {
    let lease = Lease {
        server: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9),
        local_ip: Ipv4Addr::new(10, 9, 9, 9),
        protocol: Protocol::Udp,
        local_port: 51820,
        external: SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 51820),
        lifetime: Duration::from_secs(60),
        #[cfg(feature = "restart-announcements")]
        epoch: Epoch::new(9),
    };
    assert!(matches!(
        renew(&lease, 60).await,
        Err(Failure::NetworkChanged)
    ));
}

#[test]
fn result_names() {
    for code in 1..=5 {
        let known = matches!(refused(code), Failure::Refused { name, .. } if name != "unknown");
        assert!(known, "{code}");
    }
    assert!(matches!(
        refused(6),
        Failure::Refused {
            name: "unknown",
            ..
        }
    ));
}
