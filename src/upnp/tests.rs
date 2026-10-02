use super::*;

#[tokio::test]
async fn renewal_from_another_address() {
    let service = Service {
        kind: "urn:schemas-upnp-org:service:WANIPConnection:1",
        control: Url::parse("http://127.0.0.1:9/ctl").unwrap(),
    };
    let lease = Lease {
        service,
        local_ip: Ipv4Addr::new(10, 9, 9, 9),
        protocol: Protocol::Udp,
        local_port: 51820,
        external: SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 5), 51820),
        lease_secs: 60,
    };
    assert!(matches!(
        renew(&lease, "test").await,
        Err(Failure::NetworkChanged)
    ));
}

#[test]
fn random_ports_are_not_privileged() {
    let ports: Vec<u16> = (0..1000).map(|_| random_port().unwrap()).collect();
    assert!(ports.iter().all(|p| *p >= 1024));
    // Spread over the range: no port under 5000, or none over 60000, has
    // a chance of less than 2^-90.
    assert!(ports.iter().any(|p| *p < 5000) && ports.iter().any(|p| *p > 60000));
}
