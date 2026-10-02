//! The gateway's device description: its WAN connection services.

use super::url::Url;
use crate::error::{Failure, Result};

/// The services that can map ports, best first. Version 2 of
/// WANIPConnection is in IGD version 2.
const SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

/// Services beyond this many are ignored.
const MAX_SERVICES: usize = 8;

/// A WAN connection service of a gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Service {
    /// The service type, for SOAP calls.
    pub(crate) kind: &'static str,
    pub(crate) control: Url,
}

/// The WAN connection services in a description fetched from `location`,
/// best first. Only services on the same host are kept: the description
/// comes from the network, and must not send our requests elsewhere.
pub(crate) fn services(xml: &str, location: &Url) -> Result<Vec<Service>> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))
        .map_err(|_| Failure::BadReply("bad device description"))?;
    // UPnP 1.0 descriptions may give a base for relative URLs.
    let base = child_text(doc.root_element(), "URLBase")
        .and_then(Url::parse)
        .filter(|base| base.addr.ip() == location.addr.ip())
        .unwrap_or_else(|| location.clone());
    let mut found = Vec::new();
    for node in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "service")
    {
        let Some(kind) = child_text(node, "serviceType") else {
            continue;
        };
        let Some(rank) = SERVICES.iter().position(|s| *s == kind) else {
            continue;
        };
        let Some(control) = child_text(node, "controlURL").and_then(|c| base.join(c)) else {
            continue;
        };
        if control.addr.ip() == location.addr.ip() {
            found.push((
                rank,
                Service {
                    kind: SERVICES[rank],
                    control,
                },
            ));
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    // Each service can cost seconds to try. A gateway has one or two.
    found.truncate(MAX_SERVICES);
    Ok(found.into_iter().map(|(_, service)| service).collect())
}

/// The trimmed text of the first child element called `name`.
fn child_text<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)?
        .text()
        .map(str::trim)
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
