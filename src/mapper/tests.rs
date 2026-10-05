use super::*;

#[test]
fn a_new_gateway_is_a_new_network() {
    let targets = Targets {
        gateway: Gateway::Fixed(Ipv4Addr::new(192, 168, 1, 1)),
        ..Targets::system()
    };
    let server = |ip| SocketAddrV4::new(ip, udp::PMP_PORT);
    assert!(same_gateway(&targets, server(Ipv4Addr::new(192, 168, 1, 1))).is_ok());
    let moved = same_gateway(&targets, server(Ipv4Addr::new(10, 0, 0, 1)));
    assert!(matches!(moved, Err(Failure::NetworkChanged)));
}

/// The system's own lookup. The host may have no default route, so either
/// answer is right, but the routing table must be read.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_system_gateway() {
    match Targets::system().gateway() {
        Ok(gateway) => assert!(!gateway.is_unspecified()),
        Err(Failure::NoDefaultGateway(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
        }
        Err(e) => panic!("{e}"),
    }
}
