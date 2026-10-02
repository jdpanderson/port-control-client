//! The public API, as a user of the crate sees it. No test here sends
//! anything to the network.

use std::{num::NonZeroU16, time::Duration};

use port_control_client::{
    Cause, Config, Error, ErrorKind, Mapping, Method, PortMapping, Protocol,
};

fn port() -> NonZeroU16 {
    NonZeroU16::new(51820).unwrap()
}

/// A config that tries no protocol, so nothing goes to the network.
fn offline() -> Config {
    Config::new(Protocol::Udp, port())
        .pcp(false)
        .nat_pmp(false)
        .upnp(false)
}

fn thread_safe<T: Send + Sync + Unpin + 'static>() {}

#[test]
fn types_are_thread_safe() {
    thread_safe::<PortMapping>();
    thread_safe::<Config>();
    thread_safe::<Mapping>();
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

#[tokio::test]
async fn last_error_says_why() {
    let mapping = PortMapping::start(offline());
    let mut tries = 0;
    let error = loop {
        if let Some(error) = mapping.last_error() {
            break error;
        }
        tries += 1;
        assert!(tries < 100, "no error after one second");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let [cause] = error.causes() else {
        panic!("{error:?}");
    };
    assert_eq!(cause.method(), None);
    assert_eq!(cause.kind(), ErrorKind::NoProtocol);
    assert_eq!(error.to_string(), "PCP, NAT-PMP and UPnP are all off");
    mapping.stop().await;
}

#[tokio::test]
async fn stop_can_run_on_another_task() {
    let mapping = PortMapping::start(offline());
    assert_eq!(mapping.mapping(), None);
    tokio::spawn(async move { mapping.stop().await })
        .await
        .unwrap();
}
