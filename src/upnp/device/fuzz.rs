//! For the fuzz target: any text as a device description. Every service
//! must be on the host that gave the description.

use super::{MAX_SERVICES, services};
use crate::upnp::url::Url;

pub(crate) fn run(data: &[u8]) {
    let location = Url::parse("http://192.168.1.1:5000/rootDesc.xml").expect("a valid URL");
    if let Ok(found) = services(&String::from_utf8_lossy(data), &location) {
        assert!(found.len() <= MAX_SERVICES);
        for service in found {
            assert_eq!(service.control.addr.ip(), location.addr.ip());
        }
    }
}
