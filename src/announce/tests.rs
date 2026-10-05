use super::*;

const PCP_ANNOUNCE: [u8; 24] = [
    2, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0x03, 0xe8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const NAT_PMP_ANNOUNCE: [u8; 12] = [0, 128, 0, 0, 0, 0, 0x03, 0xe8, 203, 0, 113, 5];

#[test]
fn announcements() {
    assert_eq!(parse(&PCP_ANNOUNCE, Kind::Pcp), Some(1000));
    assert_eq!(parse(&NAT_PMP_ANNOUNCE, Kind::NatPmp), Some(1000));
    // Each kind only.
    assert_eq!(parse(&PCP_ANNOUNCE, Kind::NatPmp), None);
    assert_eq!(parse(&NAT_PMP_ANNOUNCE, Kind::Pcp), None);
}

#[test]
fn not_announcements() {
    let with = |mut b: Vec<u8>, at: usize, value: u8| {
        b[at] = value;
        b
    };
    let cases = [
        // Short.
        (PCP_ANNOUNCE[..23].to_vec(), Kind::Pcp),
        (NAT_PMP_ANNOUNCE[..11].to_vec(), Kind::NatPmp),
        // An error result.
        (with(PCP_ANNOUNCE.to_vec(), 3, 1), Kind::Pcp),
        (with(NAT_PMP_ANNOUNCE.to_vec(), 3, 3), Kind::NatPmp),
        // A MAP response.
        (with(PCP_ANNOUNCE.to_vec(), 1, 0x81), Kind::Pcp),
    ];
    for (b, kind) in cases {
        assert_eq!(parse(&b, kind), None, "{kind:?} {b:?}");
    }
}

#[test]
fn epochs() {
    // The kind, the seconds since an epoch time of 1000, the next epoch
    // time, and whether it shows a lost state.
    let cases = [
        // No time passed.
        (Kind::Pcp, 0, 1000, false),
        (Kind::NatPmp, 0, 1000, false),
        // Both clocks move together, or nearly so.
        (Kind::Pcp, 100, 1100, false),
        (Kind::Pcp, 100, 1095, false),
        // Back by one second, as when replies come in another order.
        (Kind::Pcp, 0, 999, false),
        // A restart.
        (Kind::Pcp, 0, 998, true),
        (Kind::Pcp, 100, 3, true),
        // The server's clock runs too fast, or too slow.
        (Kind::Pcp, 100, 1200, true),
        (Kind::Pcp, 100, 1050, true),
        // Exactly at each limit, the epoch time is still valid.
        // 98 + 2 = 106 - 106 / 16
        (Kind::Pcp, 98, 1106, false),
        // 150 + 2 = 152, not less than 160 - 160 / 16 = 150
        (Kind::Pcp, 150, 1160, false),
        // 103 + 2 = 112 - 112 / 16
        (Kind::Pcp, 112, 1103, false),
        // 149 + 2 = 151, not less than 160 - 160 / 16 = 150
        (Kind::Pcp, 160, 1149, false),
        // 7/8 of 80 seconds is 70: up to 2 seconds less is still valid.
        (Kind::NatPmp, 80, 1080, false),
        (Kind::NatPmp, 80, 1068, false),
        (Kind::NatPmp, 80, 1067, true),
        (Kind::NatPmp, 0, 0, true),
    ];
    let epoch = Epoch::new(1000);
    for (kind, secs, next, want) in cases {
        let got = epoch.lost(kind, next, epoch.at + Duration::from_secs(secs));
        assert_eq!(got, want, "{kind:?} after {secs} s: {next}");
    }
}

#[test]
fn extreme_epochs() {
    let epoch = Epoch::new(u32::MAX);
    // About 500 years.
    let later = epoch.at + Duration::from_secs(u64::from(u32::MAX) * 4);
    for kind in [Kind::Pcp, Kind::NatPmp] {
        assert!(epoch.lost(kind, 0, later));
    }
}

/// The task listens on systems where it finds the default gateway.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn joins_the_group() {
    let any = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    Listener::bind(any, Some(GROUP), Ipv4Addr::UNSPECIFIED).unwrap();
}

#[tokio::test]
async fn listens_to_the_server_only() {
    let listener = Listener::bind(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
        None,
        Ipv4Addr::LOCALHOST,
    )
    .unwrap();
    let to = listener.socket.local_addr().unwrap();
    let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let other = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let server_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, server.local_addr().unwrap().port());
    let mut from_server = PCP_ANNOUNCE;
    from_server[11] = 7;
    other.send_to(&PCP_ANNOUNCE, to).await.unwrap();
    server.send_to(b"not an announcement", to).await.unwrap();
    server.send_to(&from_server, to).await.unwrap();
    let epoch = listener.next(server_addr, Kind::Pcp).await.unwrap();
    assert_eq!(epoch, 0x0307);
}
