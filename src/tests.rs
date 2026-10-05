//! The whole task against fake routers: get, renew, retry and release.

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    num::NonZeroU16,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use tokio::{
    net::UdpSocket,
    time::{Duration, Instant, sleep, timeout},
};

use crate::{
    Config, ErrorKind, Mapping, Method, PortMapping, Protocol,
    fake::{EXTERNAL_IP, FakeIgd, FakePmp, GRANTED_PORT, IgdCall, IgdOptions, PmpRequest, Speaks},
    mapper::{Gateway, Targets},
};

const PORT: u16 = 51820;

fn config() -> Config {
    Config::new(Protocol::Udp, NonZeroU16::new(PORT).unwrap())
}

fn targets(pmp_port: u16, ssdp: SocketAddrV4) -> Targets {
    Targets {
        gateway: Gateway::Fixed(Ipv4Addr::LOCALHOST),
        pmp_port,
        ssdp,
        ssdp_wait: Duration::from_millis(300),
        // Two sends: a silent PCP server costs about 1.75 s, as a silent
        // NAT-PMP server does.
        pcp_wait: Duration::from_millis(875),
        #[cfg(feature = "restart-announcements")]
        announce: crate::announce::Targets {
            addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
            group: None,
            delay: Duration::ZERO,
        },
    }
}

/// A local UDP port with nothing behind it.
async fn closed_port() -> u16 {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    socket.local_addr().unwrap().port()
}

async fn no_ssdp() -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, closed_port().await)
}

async fn granted(mapping: &PortMapping) -> Mapping {
    let mut watch = mapping.watch();
    let granted = timeout(Duration::from_secs(10), watch.wait_for(Option::is_some))
        .await
        .expect("no mapping in time")
        .unwrap();
    granted.unwrap()
}

/// Waits up to 10 seconds for `done`.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn pcp_maps_renews_and_releases() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(2))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    let external = SocketAddrV4::new(EXTERNAL_IP, GRANTED_PORT);
    assert_eq!(
        granted(&mapping).await,
        Mapping {
            external,
            method: Method::Pcp
        }
    );

    // Renewed at half the lifetime: the same nonce, and the granted port.
    until("a renewal", || router.seen().len() >= 2).await;
    mapping.stop().await;
    assert_eq!(mapping.mapping(), None);
    let seen = router.requests();
    let PmpRequest::PcpMap {
        nonce,
        lifetime: 2,
        suggested_port: 0,
    } = seen[0]
    else {
        panic!("{seen:?}");
    };
    let renewal = PmpRequest::PcpMap {
        nonce,
        lifetime: 2,
        suggested_port: GRANTED_PORT,
    };
    assert_eq!(seen[1], renewal);
    // A delete has no suggested port (RFC 6887, section 15).
    let release = PmpRequest::PcpMap {
        nonce,
        lifetime: 0,
        suggested_port: 0,
    };
    assert_eq!(seen.last(), Some(&release));
}

#[tokio::test]
async fn nat_pmp_when_pcp_is_not_supported() {
    let router = FakePmp::start(Speaks::NatPmp).await;
    let mapping = PortMapping::start_with(
        config().methods([Method::Pcp, Method::NatPmp]),
        targets(router.port, no_ssdp().await),
    );
    let external = SocketAddrV4::new(EXTERNAL_IP, PORT);
    assert_eq!(
        granted(&mapping).await,
        Mapping {
            external,
            method: Method::NatPmp
        }
    );
    mapping.stop().await;

    let seen = router.requests();
    assert!(matches!(seen[0], PmpRequest::PcpMap { .. }), "{seen:?}");
    assert_eq!(
        seen[1..],
        [
            PmpRequest::NatPmpExternal,
            PmpRequest::NatPmpMap {
                lifetime: 7200,
                suggested_port: PORT
            },
            PmpRequest::NatPmpMap {
                lifetime: 0,
                suggested_port: 0
            },
        ]
    );
}

#[tokio::test]
async fn nat_pmp_maps_only_with_an_external_address() {
    let router = FakePmp::start(Speaks::NatPmp).await;
    router.set_external(Ipv4Addr::UNSPECIFIED);
    let mapping = PortMapping::start_with(
        config().methods([Method::Pcp, Method::NatPmp]),
        targets(router.port, no_ssdp().await),
    );
    until("the error", || mapping.last_error().is_some()).await;
    assert_eq!(
        causes(&mapping).last(),
        Some(&(Some(Method::NatPmp), ErrorKind::BadReply))
    );
    // So there is no mapping on the router to delete.
    let maps = router
        .requests()
        .into_iter()
        .filter(|r| matches!(r, PmpRequest::NatPmpMap { .. }))
        .count();
    assert_eq!(maps, 0);
}

#[tokio::test]
async fn upnp_when_nothing_answers_on_the_pmp_port() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let mapping = PortMapping::start_with(config(), targets(closed_port().await, gateway.ssdp));
    let external = SocketAddrV4::new(EXTERNAL_IP, PORT);
    assert_eq!(
        granted(&mapping).await,
        Mapping {
            external,
            method: Method::Upnp
        }
    );
    mapping.stop().await;

    let add = IgdCall::Add {
        external_port: PORT,
        internal_port: PORT,
        client: "127.0.0.1".to_owned(),
        lease: 7200,
    };
    let delete = IgdCall::Delete {
        external_port: PORT,
    };
    assert_eq!(gateway.calls(), [IgdCall::GetExternal, add, delete]);
}

#[tokio::test]
async fn no_nat_pmp_by_default() {
    let router = FakePmp::start(Speaks::NatPmp).await;
    let mapping = PortMapping::start_with(config(), targets(router.port, no_ssdp().await));
    until("the error", || mapping.last_error().is_some()).await;
    let methods: Vec<_> = causes(&mapping).into_iter().map(|(m, _)| m).collect();
    assert_eq!(methods, [Some(Method::Pcp), Some(Method::Upnp)]);
    let seen = router.requests();
    assert!(matches!(seen[..], [PmpRequest::PcpMap { .. }]), "{seen:?}");
}

#[tokio::test]
async fn tries_methods_in_the_configured_order() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let config = config().methods([Method::Upnp, Method::Pcp]);
    let mapping = PortMapping::start_with(config, targets(router.port, gateway.ssdp));
    assert_eq!(granted(&mapping).await.method, Method::Upnp);
    mapping.stop().await;
    assert_eq!(router.requests(), []);
}

#[tokio::test]
async fn upnp_permanent_leases_and_taken_ports() {
    let options = IgdOptions {
        permanent_only: true,
        taken: vec![PORT],
        chunked: true,
        ..IgdOptions::default()
    };
    let gateway = FakeIgd::start(options).await;
    let config = config().methods([Method::Upnp]);
    let mapping = PortMapping::start_with(config, targets(closed_port().await, gateway.ssdp));
    let granted = granted(&mapping).await;
    assert_eq!(granted.method, Method::Upnp);
    assert_ne!(granted.external.port(), PORT);

    let ports_and_leases: Vec<_> = gateway
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            IgdCall::Add {
                external_port,
                lease,
                ..
            } => Some((external_port, lease)),
            _ => None,
        })
        .collect();
    let other = granted.external.port();
    assert_eq!(ports_and_leases, [(PORT, 7200), (PORT, 0), (other, 0)]);
}

#[tokio::test]
async fn retries_after_the_interval() {
    let router = FakePmp::start(Speaks::Silent).await;
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .retry_interval(Duration::from_secs(1));
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    // Two sends for PCP and three for NAT-PMP, with no answer.
    until("the first attempt", || router.seen().len() >= 5).await;
    router.speak(Speaks::Pcp);
    assert_eq!(granted(&mapping).await.method, Method::Pcp);
}

#[tokio::test]
async fn refresh_asks_again_at_once() {
    let router = FakePmp::start(Speaks::Silent).await;
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .retry_interval(Duration::from_secs(3600));
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    until("the first attempt", || router.seen().len() >= 5).await;
    // The last NAT-PMP send waits one second for a reply.
    sleep(Duration::from_millis(1100)).await;
    router.speak(Speaks::Pcp);
    sleep(Duration::from_millis(300)).await;
    assert_eq!(mapping.mapping(), None);
    mapping.refresh();
    assert_eq!(granted(&mapping).await.method, Method::Pcp);
}

#[tokio::test]
async fn keeps_the_mapping_until_it_expires() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(2))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    let start = Instant::now();
    router.speak(Speaks::Silent);
    // The renewal at 1 s waits for a reply until about 2.75 s, but the
    // mapping lasts only until 2 s.
    sleep(Duration::from_millis(1500)).await;
    assert!(mapping.mapping().is_some());
    until("the mapping to expire", || mapping.mapping().is_none()).await;
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(1900), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(2400), "{elapsed:?}");
}

#[tokio::test]
async fn clears_the_mapping_at_expiry_between_steps() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(4))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    let start = Instant::now();
    router.speak(Speaks::Silent);
    // The renewal at 2 s fails at about 3.75 s. The next step is 1 s later,
    // after the expiry at 4 s.
    until("the mapping to expire", || mapping.mapping().is_none()).await;
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(3800), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(4500), "{elapsed:?}");
}

/// Targets that listen for restart announcements on a free local port.
#[cfg(feature = "restart-announcements")]
async fn listening(pmp_port: u16) -> (Targets, SocketAddrV4) {
    let mut targets = targets(pmp_port, no_ssdp().await);
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, closed_port().await);
    targets.announce.addr = addr;
    (targets, addr)
}

/// How many map requests of the router's protocol it has seen.
#[cfg(feature = "restart-announcements")]
fn maps(router: &FakePmp, speaks: Speaks) -> usize {
    let requests = router.requests();
    let is_map = |r: &&PmpRequest| match speaks {
        Speaks::Pcp => matches!(r, PmpRequest::PcpMap { lifetime: 7200, .. }),
        _ => matches!(r, PmpRequest::NatPmpMap { lifetime: 7200, .. }),
    };
    requests.iter().filter(is_map).count()
}

#[cfg(feature = "restart-announcements")]
#[tokio::test]
async fn renews_after_a_router_restart() {
    for speaks in [Speaks::Pcp, Speaks::NatPmp] {
        let router = FakePmp::start(speaks).await;
        let (targets, announce) = listening(router.port).await;
        let config = config()
            .methods([Method::Pcp, Method::NatPmp])
            .restart_announcements(true);
        let mapping = PortMapping::start_with(config, targets);
        granted(&mapping).await;
        assert_eq!(maps(&router, speaks), 1);
        router.restart();
        router.announce(announce).await;
        until("the renewal", || maps(&router, speaks) == 2).await;
    }
}

#[cfg(feature = "restart-announcements")]
#[tokio::test]
async fn listens_only_when_turned_on() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let (targets, announce) = listening(router.port).await;
    let mapping = PortMapping::start_with(config().methods([Method::Pcp, Method::NatPmp]), targets);
    granted(&mapping).await;
    router.restart();
    router.announce(announce).await;
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(maps(&router, Speaks::Pcp), 1);
}

#[cfg(feature = "restart-announcements")]
#[tokio::test]
async fn maps_without_a_restart_listener() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let mut targets = targets(router.port, no_ssdp().await);
    // An address that is not on this host: the listener can't open.
    targets.announce.addr = SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 0);
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .restart_announcements(true);
    let mapping = PortMapping::start_with(config, targets);
    assert_eq!(granted(&mapping).await.method, Method::Pcp);
}

#[cfg(feature = "restart-announcements")]
#[tokio::test]
async fn ignores_other_announcements() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let (targets, announce) = listening(router.port).await;
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .restart_announcements(true);
    let mapping = PortMapping::start_with(config, targets);
    granted(&mapping).await;
    // An announcement with no restart, and a restart announcement from
    // another host.
    router.announce(announce).await;
    let other = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut restart = vec![2, 0x80, 0, 0];
    restart.resize(24, 0);
    other.send_to(&restart, announce).await.unwrap();
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(maps(&router, Speaks::Pcp), 1);
}

#[tokio::test]
async fn a_new_gateway_ends_the_mapping() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let gateway = Arc::new(AtomicU32::new(Ipv4Addr::LOCALHOST.into()));
    let targets = Targets {
        gateway: Gateway::Changing(gateway.clone()),
        ..targets(router.port, no_ssdp().await)
    };
    let mapping = PortMapping::start_with(config().methods([Method::Pcp, Method::NatPmp]), targets);
    granted(&mapping).await;
    // Another network, with no router at its gateway. The mapping ends
    // long before it expires, and no renewal goes to the old gateway.
    gateway.store(Ipv4Addr::new(127, 0, 0, 2).into(), Ordering::SeqCst);
    mapping.refresh();
    until("the loss", || mapping.mapping().is_none()).await;
    assert_eq!(router.requests().len(), 1);
}

#[tokio::test]
async fn dropping_the_handle_releases() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let mapping = PortMapping::start_with(
        config().methods([Method::Pcp, Method::NatPmp]),
        targets(router.port, no_ssdp().await),
    );
    granted(&mapping).await;
    drop(mapping);
    until("the release", || {
        matches!(
            router.seen().last(),
            Some(PmpRequest::PcpMap { lifetime: 0, .. })
        )
    })
    .await;
}

#[tokio::test]
async fn nothing_to_try() {
    let config = config().methods([]);
    let mapping = PortMapping::start_with(config, targets(closed_port().await, no_ssdp().await));
    sleep(Duration::from_millis(100)).await;
    assert_eq!(mapping.mapping(), None);
    timeout(Duration::from_secs(1), mapping.stop())
        .await
        .unwrap();
}

#[tokio::test]
async fn one_pcp_nonce_for_every_attempt() {
    // The server stays silent, so the task asks twice for a new mapping.
    let router = FakePmp::start(Speaks::Silent).await;
    let config = config()
        .methods([Method::Pcp])
        .retry_interval(Duration::from_secs(1));
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    // Two sends for each attempt.
    until("two attempts", || router.seen().len() >= 4).await;
    mapping.stop().await;
    let nonces: Vec<_> = router
        .requests()
        .into_iter()
        .map(|r| match r {
            PmpRequest::PcpMap { nonce, .. } => nonce,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(nonces.iter().all(|n| *n == nonces[0]), "{nonces:?}");
}

#[tokio::test]
async fn very_long_retry_interval() {
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .retry_interval(Duration::MAX);
    let mapping = PortMapping::start_with(config, targets(closed_port().await, no_ssdp().await));
    // PCP and NAT-PMP fail immediately on a closed port; the task must then
    // wait, not panic, and still stop.
    sleep(Duration::from_millis(300)).await;
    assert_eq!(mapping.mapping(), None);
    timeout(Duration::from_secs(3), mapping.stop())
        .await
        .unwrap();
}

/// The kind and method of each cause of the last error.
fn causes(mapping: &PortMapping) -> Vec<(Option<Method>, ErrorKind)> {
    let Some(error) = mapping.last_error() else {
        return Vec::new();
    };
    error
        .causes()
        .iter()
        .map(|c| (c.method(), c.kind()))
        .collect()
}

#[tokio::test]
async fn last_error_has_a_cause_for_each_protocol() {
    let router = FakePmp::start(Speaks::Silent).await;
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .retry_interval(Duration::from_secs(1));
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    assert_eq!(mapping.last_error(), None);
    until("the first error", || mapping.last_error().is_some()).await;
    assert_eq!(
        causes(&mapping),
        [
            (Some(Method::Pcp), ErrorKind::Timeout),
            (Some(Method::NatPmp), ErrorKind::Timeout),
        ]
    );
    let error = mapping.last_error().unwrap();
    assert_eq!(
        error.to_string(),
        "PCP: no reply in time; NAT-PMP: no reply in time"
    );

    // Success clears it.
    router.speak(Speaks::Pcp);
    granted(&mapping).await;
    assert_eq!(mapping.last_error(), None);
}

#[tokio::test]
async fn last_error_shows_a_refusal() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    gateway.refuse(606);
    let mapping = upnp_error(&gateway).await;
    assert_eq!(causes(&mapping), [refused(606)]);
    assert_eq!(mapping.mapping(), None);
}

#[tokio::test]
async fn retries_a_failed_renewal_before_the_expiry() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(8))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    router.speak(Speaks::Silent);
    // The renewal at 4 s fails at about 5.75 s. The next try is at half
    // the time left, about 6.9 s, before the expiry at 8 s.
    until("the renewal error", || mapping.last_error().is_some()).await;
    router.speak(Speaks::Pcp);
    until("the next try", || {
        assert!(mapping.mapping().is_some(), "the mapping expired");
        mapping.last_error().is_none()
    })
    .await;
}

#[tokio::test]
async fn each_task_has_its_own_nonce() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config().methods([Method::Pcp, Method::NatPmp]);
    let first = PortMapping::start_with(config.clone(), targets(router.port, no_ssdp().await));
    granted(&first).await;
    let second = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&second).await;
    let nonces: Vec<_> = router
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            PmpRequest::PcpMap { nonce, .. } => Some(nonce),
            _ => None,
        })
        .collect();
    assert_eq!(nonces.len(), 2, "{nonces:?}");
    assert_ne!(nonces[0], nonces[1]);
}

#[test]
fn random_bytes_differ() {
    // Two 12-byte values are the same with a chance of 2^-96.
    assert_ne!(
        crate::random::<12>().unwrap(),
        crate::random::<12>().unwrap()
    );
}

#[test]
fn random_fractions_are_from_0_to_1() {
    let values: Vec<f64> = (0..100).map(|_| crate::random_fraction()).collect();
    assert!(values.iter().all(|v| (0.0..=1.0).contains(v)), "{values:?}");
    // All 100 on one side of 0.5 has a chance of 2^-99.
    assert!(values.iter().any(|v| *v < 0.5) && values.iter().any(|v| *v > 0.5));
}

#[tokio::test]
async fn last_error_while_renewal_fails() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(6))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    router.speak(Speaks::Silent);
    // The renewal at 3 s fails; the mapping stays until 6 s.
    until("the renewal error", || mapping.last_error().is_some()).await;
    assert!(mapping.mapping().is_some());
    assert_eq!(causes(&mapping), [(Some(Method::Pcp), ErrorKind::Timeout)]);
}

#[tokio::test]
async fn last_error_after_a_failed_release() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let mapping = PortMapping::start_with(
        config().methods([Method::Pcp, Method::NatPmp]),
        targets(router.port, no_ssdp().await),
    );
    granted(&mapping).await;
    router.speak(Speaks::Silent);
    mapping.stop().await;
    assert_eq!(causes(&mapping), [(Some(Method::Pcp), ErrorKind::Timeout)]);
}

#[tokio::test]
async fn last_error_when_every_protocol_is_off() {
    let config = config().methods([]);
    let mapping = PortMapping::start_with(config, targets(closed_port().await, no_ssdp().await));
    until("the error", || mapping.last_error().is_some()).await;
    assert_eq!(causes(&mapping), [(None, ErrorKind::NoProtocol)]);
}

#[test]
fn config_limits() {
    let config = config()
        .lifetime(Duration::ZERO)
        .retry_interval(Duration::MAX);
    assert_eq!(config.lifetime_secs(), 1);
    assert_eq!(config.retry_wait(), Duration::from_secs(u32::MAX.into()));
    let config = self::config().lifetime(Duration::MAX);
    assert_eq!(config.lifetime_secs(), u32::MAX);
}

#[tokio::test]
async fn refresh_during_a_step_runs_the_next_step_at_once() {
    let router = FakePmp::start(Speaks::Silent).await;
    let config = config()
        .methods([Method::Pcp, Method::NatPmp])
        .retry_interval(Duration::from_secs(3600));
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    // PCP has stopped after two sends; NAT-PMP is still trying, and gets
    // no answer from a PCP-only server.
    until("NAT-PMP", || router.seen().len() >= 3).await;
    router.speak(Speaks::Pcp);
    mapping.refresh();
    assert_eq!(granted(&mapping).await.method, Method::Pcp);
}

#[tokio::test]
async fn renewal_with_a_new_external_address() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(2))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    let ip = Ipv4Addr::new(198, 51, 100, 7);
    router.set_external(ip);
    let mut watch = mapping.watch();
    let changed = timeout(
        Duration::from_secs(10),
        watch.wait_for(|m| m.is_some_and(|m| *m.external.ip() == ip)),
    )
    .await
    .expect("no new address in time")
    .unwrap();
    assert_eq!(
        changed.unwrap().external,
        SocketAddrV4::new(ip, GRANTED_PORT)
    );
}

#[tokio::test]
async fn a_refused_renewal_ends_the_mapping_at_once() {
    let router = FakePmp::start(Speaks::Pcp).await;
    let config = config()
        .lifetime(Duration::from_secs(6))
        .methods([Method::Pcp, Method::NatPmp])
        .retry_interval(Duration::from_secs(3600));
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    let start = Instant::now();
    router.speak(Speaks::Refuses);
    until("the loss", || mapping.mapping().is_none()).await;
    // At the renewal (3 s), before the lifetime ends (6 s).
    assert!(start.elapsed() < Duration::from_secs(5));
    // The task asks again immediately, and both protocols are refused.
    let refused = |c: &(Option<Method>, ErrorKind)| {
        matches!(
            c.1,
            ErrorKind::Refused {
                code: 2,
                temporary: false,
                ..
            }
        )
    };
    until("the new attempt", || {
        let causes = causes(&mapping);
        causes.len() == 2 && causes.iter().all(refused)
    })
    .await;
}

#[tokio::test]
async fn nat_pmp_renews_with_the_granted_port() {
    let router = FakePmp::start(Speaks::NatPmp).await;
    let config = config()
        .lifetime(Duration::from_secs(2))
        .methods([Method::Pcp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets(router.port, no_ssdp().await));
    granted(&mapping).await;
    let maps = || {
        router
            .requests()
            .into_iter()
            .filter_map(|r| match r {
                PmpRequest::NatPmpMap {
                    lifetime: 2,
                    suggested_port,
                } => Some(suggested_port),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // The second renewal comes after the task has the first one's result.
    until("two renewals", || maps().len() >= 3).await;
    assert!(maps().iter().all(|port| *port == PORT));
    assert_eq!(mapping.last_error(), None);
}

#[tokio::test]
async fn upnp_renews_the_same_mapping() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let mapping = upnp_mapping(&gateway, 2).await;
    let adds = || {
        gateway
            .calls()
            .into_iter()
            .filter(|c| matches!(c, IgdCall::Add { .. }))
            .collect::<Vec<_>>()
    };
    // The second renewal comes after the task has the first one's result.
    until("two renewals", || adds().len() >= 3).await;
    let adds = adds();
    assert!(adds.iter().all(|add| *add == adds[0]), "{adds:?}");
    assert_eq!(mapping.last_error(), None);
}

/// A UPnP mapping with the given lifetime, after the gateway grants it.
async fn upnp_mapping(gateway: &FakeIgd, lifetime: u64) -> PortMapping {
    let config = config()
        .methods([Method::Upnp])
        .lifetime(Duration::from_secs(lifetime));
    let mapping = PortMapping::start_with(config, targets(closed_port().await, gateway.ssdp));
    granted(&mapping).await;
    mapping
}

/// The gateway's calls, short: `get`, `add <port>` or `delete <port>`.
fn upnp_calls(gateway: &FakeIgd) -> Vec<String> {
    gateway
        .calls()
        .into_iter()
        .map(|call| match call {
            IgdCall::GetExternal => "get".to_owned(),
            IgdCall::Add { external_port, .. } => format!("add {external_port}"),
            IgdCall::Delete { external_port } => format!("delete {external_port}"),
        })
        .collect()
}

#[tokio::test]
async fn upnp_deletes_a_lost_mapping() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let _mapping = upnp_mapping(&gateway, 2).await;
    gateway.refuse(606);
    let delete = IgdCall::Delete {
        external_port: PORT,
    };
    until("the delete", || gateway.calls().contains(&delete)).await;
    // The refused renewal, then the delete, which works.
    assert_eq!(
        upnp_calls(&gateway)[..4],
        ["get", "add 51820", "add 51820", "delete 51820"]
    );
}

#[tokio::test]
async fn upnp_keeps_a_mapping_through_a_temporary_failure() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let mapping = upnp_mapping(&gateway, 4).await;
    gateway.refuse(501);
    gateway.refuse_deletes(501);
    until("the renewal error", || mapping.last_error().is_some()).await;
    assert!(mapping.mapping().is_some());
    let [(method, kind)] = causes(&mapping)[..] else {
        panic!("{:?}", mapping.last_error());
    };
    assert_eq!(method, Some(Method::Upnp));
    assert!(
        matches!(
            kind,
            ErrorKind::Refused {
                code: 501,
                temporary: true,
                ..
            }
        ),
        "{kind:?}"
    );
    // At the expiry, the task deletes the mapping.
    let delete = IgdCall::Delete {
        external_port: PORT,
    };
    until("the delete", || gateway.calls().contains(&delete)).await;
    until("the loss", || mapping.mapping().is_none()).await;
}

#[tokio::test]
async fn upnp_keeps_another_hosts_mapping() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let mapping = upnp_mapping(&gateway, 2).await;
    // Another host now has the external port.
    gateway.refuse(718);
    until("the next attempt", || gateway.calls().len() >= 4).await;
    assert_eq!(mapping.mapping(), None);
    // The refused renewal, then a new attempt, with no delete.
    assert_eq!(
        upnp_calls(&gateway)[..4],
        ["get", "add 51820", "add 51820", "get"]
    );
}

#[tokio::test]
async fn upnp_renewal_with_a_new_external_address() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let mapping = upnp_mapping(&gateway, 2).await;
    let ip = Ipv4Addr::new(198, 51, 100, 7);
    gateway.set_external(ip);
    until("the new address", || {
        mapping.mapping().map(|m| m.external) == Some(SocketAddrV4::new(ip, PORT))
    })
    .await;
}

/// Starts a UPnP mapping with `gateway`, and waits for the first error.
async fn upnp_error(gateway: &FakeIgd) -> PortMapping {
    let config = config().methods([Method::Upnp]);
    let mapping = PortMapping::start_with(config, targets(closed_port().await, gateway.ssdp));
    until("the error", || mapping.last_error().is_some()).await;
    mapping
}

/// A permanent refusal from a UPnP gateway, as a cause.
fn refused(code: u16) -> (Option<Method>, ErrorKind) {
    let temporary = false;
    (Some(Method::Upnp), ErrorKind::Refused { code, temporary })
}

/// The lease of each AddPortMapping call.
fn leases(gateway: &FakeIgd) -> Vec<u32> {
    let calls = gateway.calls();
    let lease = |call| match call {
        IgdCall::Add { lease, .. } => Some(lease),
        _ => None,
    };
    calls.into_iter().filter_map(lease).collect()
}

#[tokio::test]
async fn upnp_permanent_leases_refused_too() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    gateway.refuse(725);
    let mapping = upnp_error(&gateway).await;
    // One try with a permanent lease, then the error.
    assert_eq!(leases(&gateway), [7200, 0]);
    assert_eq!(causes(&mapping), [refused(725)]);
}

#[tokio::test]
async fn upnp_tries_three_other_ports() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    gateway.refuse(718);
    let mapping = upnp_error(&gateway).await;
    // The port, then three random ports.
    assert_eq!(leases(&gateway).len(), 4);
    assert_eq!(causes(&mapping), [refused(718)]);
}

#[tokio::test]
async fn upnp_without_a_default_gateway() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    let targets = Targets {
        gateway: Gateway::Missing,
        ..targets(closed_port().await, gateway.ssdp)
    };
    let mapping = PortMapping::start_with(config(), targets);
    assert_eq!(granted(&mapping).await.method, Method::Upnp);
}

#[tokio::test]
async fn last_error_without_a_default_gateway() {
    let targets = Targets {
        gateway: Gateway::Missing,
        ..targets(closed_port().await, no_ssdp().await)
    };
    // PCP and NAT-PMP share one cause.
    let config = config().methods([Method::Pcp, Method::Upnp, Method::NatPmp]);
    let mapping = PortMapping::start_with(config, targets);
    until("the error", || mapping.last_error().is_some()).await;
    assert_eq!(
        causes(&mapping),
        [
            (None, ErrorKind::NoDefaultGateway),
            (Some(Method::Upnp), ErrorKind::NoUpnpGateway),
        ]
    );
}

#[tokio::test]
async fn last_error_for_a_missing_description() {
    let options = IgdOptions {
        missing_description: true,
        ..IgdOptions::default()
    };
    let gateway = FakeIgd::start(options).await;
    let mapping = upnp_error(&gateway).await;
    assert_eq!(
        causes(&mapping),
        [(Some(Method::Upnp), ErrorKind::HttpStatus(404))]
    );
}

#[tokio::test]
async fn last_error_for_a_gateway_with_no_external_address() {
    let gateway = FakeIgd::start(IgdOptions::default()).await;
    gateway.set_external(Ipv4Addr::UNSPECIFIED);
    let mapping = upnp_error(&gateway).await;
    assert_eq!(
        causes(&mapping),
        [(Some(Method::Upnp), ErrorKind::BadReply)]
    );
    // A service whose WAN link is down gets no mapping request.
    assert!(
        !gateway
            .calls()
            .iter()
            .any(|c| matches!(c, IgdCall::Add { .. }))
    );
}
