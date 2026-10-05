use super::*;

#[test]
fn display() {
    let error = Error::new(vec![
        Failure::Timeout.cause(Some(Method::Pcp)),
        Failure::Refused {
            code: 606,
            name: "Action not authorized",
            temporary: false,
        }
        .cause(Some(Method::Upnp)),
    ]);
    assert_eq!(
        error.to_string(),
        "PCP: no reply in time; UPnP: error 606 (Action not authorized)"
    );
    let cause = Failure::NoProtocol.cause(None);
    assert_eq!(cause.to_string(), "PCP, NAT-PMP and UPnP are all off");
    assert_eq!(cause.method(), None);
}

#[test]
fn kinds() {
    let io = io::Error::new(io::ErrorKind::NotFound, "no IPv4 default route");
    let cases = [
        (
            Failure::Io(io::ErrorKind::ConnectionRefused.into()),
            ErrorKind::Io(io::ErrorKind::ConnectionRefused),
        ),
        (Failure::NoDefaultGateway(io), ErrorKind::NoDefaultGateway),
        #[cfg(feature = "upnp")]
        (Failure::NoUpnpGateway, ErrorKind::NoUpnpGateway),
        (Failure::BadReply("x"), ErrorKind::BadReply),
        #[cfg(feature = "upnp")]
        (Failure::HttpStatus(404), ErrorKind::HttpStatus(404)),
        (Failure::NetworkChanged, ErrorKind::NetworkChanged),
        #[cfg(not(all(feature = "pcp", feature = "nat-pmp", feature = "upnp")))]
        (Failure::NotBuilt, ErrorKind::NotBuilt),
    ];
    for (failure, kind) in cases {
        assert_eq!(failure.cause(None).kind(), kind, "{failure}");
    }
    let refused = Failure::Refused {
        code: 8,
        name: "NO_RESOURCES",
        temporary: true,
    };
    assert!(!refused.mapping_gone());
    assert!(matches!(
        refused.cause(Some(Method::Pcp)).kind(),
        ErrorKind::Refused {
            code: 8,
            temporary: true,
            ..
        }
    ));
}
