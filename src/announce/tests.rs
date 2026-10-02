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
    // Short, an error result, a MAP response.
    assert_eq!(parse(&PCP_ANNOUNCE[..23], Kind::Pcp), None);
    assert_eq!(parse(&NAT_PMP_ANNOUNCE[..11], Kind::NatPmp), None);
    let mut b = PCP_ANNOUNCE;
    b[3] = 1;
    assert_eq!(parse(&b, Kind::Pcp), None);
    let mut b = PCP_ANNOUNCE;
    b[1] = 0x81;
    assert_eq!(parse(&b, Kind::Pcp), None);
    let mut b = NAT_PMP_ANNOUNCE;
    b[3] = 3;
    assert_eq!(parse(&b, Kind::NatPmp), None);
}

/// Whether `next`, `secs` seconds after an epoch time of 1000, shows a
/// lost state.
fn lost(kind: Kind, secs: u64, next: u32) -> bool {
    let epoch = Epoch::new(1000);
    epoch.lost(kind, next, epoch.at + Duration::from_secs(secs))
}

#[test]
fn pcp_epochs() {
    // Both clocks move together, or nearly so.
    assert!(!lost(Kind::Pcp, 100, 1100));
    assert!(!lost(Kind::Pcp, 100, 1095));
    // Back by one second, as when replies come in another order.
    assert!(!lost(Kind::Pcp, 0, 999));
    // A restart.
    assert!(lost(Kind::Pcp, 0, 998));
    assert!(lost(Kind::Pcp, 100, 3));
    // The server's clock runs too fast, or too slow.
    assert!(lost(Kind::Pcp, 100, 1200));
    assert!(lost(Kind::Pcp, 100, 1050));
}

#[test]
fn pcp_epoch_limits() {
    // Exactly at each limit, the epoch time is still valid.
    // 98 + 2 = 106 - 106 / 16
    assert!(!lost(Kind::Pcp, 98, 1106));
    // 150 + 2 = 152, not less than 160 - 160 / 16 = 150
    assert!(!lost(Kind::Pcp, 150, 1160));
    // 103 + 2 = 112 - 112 / 16
    assert!(!lost(Kind::Pcp, 112, 1103));
    // 149 + 2 = 151, not less than 160 - 160 / 16 = 150
    assert!(!lost(Kind::Pcp, 160, 1149));
}

#[test]
fn nat_pmp_epochs() {
    // 7/8 of 80 seconds is 70: up to 2 seconds less is still valid.
    assert!(!lost(Kind::NatPmp, 80, 1080));
    assert!(!lost(Kind::NatPmp, 80, 1068));
    assert!(lost(Kind::NatPmp, 80, 1067));
    assert!(lost(Kind::NatPmp, 0, 0));
}

#[test]
fn extreme_epochs() {
    let epoch = Epoch::new(u32::MAX);
    // About 500 years.
    let later = epoch.at + Duration::from_secs(u64::from(u32::MAX) * 4);
    for kind in [Kind::Pcp, Kind::NatPmp] {
        assert!(epoch.lost(kind, 0, later));
        assert!(!lost(kind, 0, 1000));
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
