use super::*;

const HEADER: &str =
    "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT";

fn table(rows: &[&str]) -> String {
    std::iter::once(HEADER)
        .chain(rows.iter().copied())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lowest_metric_default_route() {
    let t = table(&[
        "eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0",
        "wlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0",
        "eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0",
    ]);
    assert_eq!(parse(&t).unwrap(), Ipv4Addr::new(192, 168, 1, 1));
}

#[test]
fn not_default_routes() {
    let t = table(&[
        // 0.0.0.0/1 through a VPN, as some VPNs add.
        "tun0\t00000000\t0100000A\t0003\t0\t0\t0\t00000080\t0\t0\t0",
        // A default route that is down.
        "eth0\t00000000\t0101A8C0\t0002\t0\t0\t100\t00000000\t0\t0\t0",
        // A default route with no gateway (point-to-point).
        "wg0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0",
        "short line",
    ]);
    assert_eq!(parse(&t).unwrap_err().kind(), io::ErrorKind::NotFound);
    assert_eq!(parse("").unwrap_err().kind(), io::ErrorKind::NotFound);
}
