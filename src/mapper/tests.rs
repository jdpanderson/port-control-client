use super::*;

#[test]
fn a_new_gateway_is_a_new_network() {
    let targets = Targets {
        gateway: Gateway::Fixed(Ipv4Addr::new(192, 168, 1, 1)),
        ..Targets::system()
    };
    let server = |ip| SocketAddrV4::new(ip, pcp::PORT);
    assert!(same_gateway(&targets, server(Ipv4Addr::new(192, 168, 1, 1))).is_ok());
    let moved = same_gateway(&targets, server(Ipv4Addr::new(10, 0, 0, 1)));
    assert!(matches!(moved, Err(Failure::NetworkChanged)));
}
