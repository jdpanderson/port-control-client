//! The public API, as a user of the crate sees it. No test here sends
//! anything to the network.

use std::{num::NonZeroU16, time::Duration};

use port_control_client::{
    Cause, Config, Error, ErrorKind, Mapping, Method, PortMapping, Protocol, Status,
};

fn port() -> NonZeroU16 {
    NonZeroU16::new(51820).unwrap()
}

/// A config that tries no protocol, so nothing goes to the network.
fn offline() -> Config {
    Config::new(Protocol::Udp, port()).methods([])
}

fn thread_safe<T: Send + Sync + Unpin + 'static>() {}

#[test]
fn types_are_thread_safe() {
    thread_safe::<PortMapping>();
    thread_safe::<Config>();
    thread_safe::<Mapping>();
    thread_safe::<Status>();
    thread_safe::<Method>();
    thread_safe::<Protocol>();
    thread_safe::<Error>();
    thread_safe::<Cause>();
    thread_safe::<ErrorKind>();
}

fn std_error<T: std::error::Error + Clone + Eq>() {}

#[test]
fn errors_are_std_errors() {
    std_error::<Error>();
    std_error::<Cause>();
}

#[test]
fn display() {
    assert_eq!(Protocol::Udp.to_string(), "UDP");
    assert_eq!(Protocol::Tcp.to_string(), "TCP");
    assert_eq!(Method::Pcp.to_string(), "PCP");
    assert_eq!(Method::NatPmp.to_string(), "NAT-PMP");
    assert_eq!(Method::Upnp.to_string(), "UPnP");
}

#[test]
fn config_builder() {
    let default = Config::new(Protocol::Udp, port());
    assert_eq!(default, Config::new(Protocol::Udp, port()));
    let changed = default
        .clone()
        .lifetime(Duration::from_secs(600))
        .retry_interval(Duration::from_secs(10))
        .description("test");
    assert_ne!(changed, default);
}

#[test]
fn method_order() {
    let default = Config::new(Protocol::Udp, port());
    let built: Vec<_> = [
        #[cfg(feature = "pcp")]
        Method::Pcp,
        #[cfg(feature = "upnp")]
        Method::Upnp,
    ]
    .into();
    assert_eq!(Config::DEFAULT_METHODS, built);
    let upnp_first =
        default
            .clone()
            .methods([Method::Upnp, Method::Pcp, Method::Upnp, Method::NatPmp]);
    assert_ne!(upnp_first, default);
    assert_eq!(
        upnp_first,
        default
            .clone()
            .methods([Method::Upnp, Method::Pcp, Method::NatPmp])
    );
    let defaults = Config::DEFAULT_METHODS.iter().copied();
    assert_eq!(default.clone().methods(defaults), default);
}

#[tokio::test]
async fn stop_twice() {
    let mapping = PortMapping::start(offline());
    mapping.stop().await;
    // The task has ended: these return immediately.
    tokio::time::timeout(Duration::from_secs(1), mapping.stop())
        .await
        .unwrap();
    mapping.refresh();
}

/// The first error of the task. The configs here fail at once.
async fn first_error(mapping: &PortMapping) -> Error {
    for _ in 0..100 {
        if let Some(error) = mapping.last_error() {
            return error;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no error after one second");
}

#[tokio::test]
async fn last_error_says_why() {
    let mapping = PortMapping::start(offline());
    let error = first_error(&mapping).await;
    let [cause] = error.causes() else {
        panic!("{error:?}");
    };
    assert_eq!(cause.method(), None);
    assert_eq!(cause.kind(), ErrorKind::NoProtocol);
    assert_eq!(error.to_string(), "PCP, NAT-PMP and UPnP are all off");
    mapping.stop().await;
}

#[tokio::test]
async fn status_holds_the_mapping_and_the_error_together() {
    let mapping = PortMapping::start(offline());
    assert_eq!(mapping.local_port(), port());
    // The task has not run yet: this test's runtime has one thread.
    assert_eq!(mapping.status(), Status::Unmapped { error: None });
    let error = first_error(&mapping).await;
    let status = mapping.status();
    assert_eq!(
        status,
        Status::Unmapped {
            error: Some(error.clone())
        }
    );
    assert_eq!((status.mapping(), status.error()), (None, Some(&error)));
    // With no mapping to release, the last error stays.
    mapping.stop().await;
    assert_eq!(mapping.status(), Status::Stopped { error: Some(error) });
}

#[tokio::test]
async fn stop_can_run_on_another_task() {
    let mapping = PortMapping::start(offline());
    assert_eq!(mapping.mapping(), None);
    tokio::spawn(async move { mapping.stop().await })
        .await
        .unwrap();
}

/// A protocol whose feature is off fails without going to the network.
#[cfg(not(all(feature = "pcp", feature = "nat-pmp", feature = "upnp")))]
#[tokio::test]
async fn protocols_not_built() {
    let not_built = [
        #[cfg(not(feature = "pcp"))]
        Method::Pcp,
        #[cfg(not(feature = "nat-pmp"))]
        Method::NatPmp,
        #[cfg(not(feature = "upnp"))]
        Method::Upnp,
    ];
    let mapping = PortMapping::start(offline().methods(not_built));
    let error = first_error(&mapping).await;
    let causes: Vec<_> = error
        .causes()
        .iter()
        .map(|c| (c.method(), c.kind()))
        .collect();
    let expected: Vec<_> = not_built
        .iter()
        .map(|&m| (Some(m), ErrorKind::NotBuilt))
        .collect();
    assert_eq!(causes, expected);
    mapping.stop().await;
}
