//! Entry points for the fuzz targets in `fuzz/`. They exist only in builds
//! with `--cfg fuzzing`, which `cargo fuzz` sets, and are not part of the
//! public API.
//!
//! Each one feeds bytes from the fuzzer to a parser of data from the
//! network, and checks what must hold for any input.

/// PCP MAP responses.
#[cfg(feature = "pcp")]
pub fn pcp_reply(data: &[u8]) {
    crate::pcp::fuzz::run(data);
}

/// NAT-PMP mapping and external address responses.
#[cfg(feature = "nat-pmp")]
pub fn nat_pmp_reply(data: &[u8]) {
    crate::nat_pmp::fuzz::run(data);
}

/// PCP and NAT-PMP restart announcements, and their epoch checks.
#[cfg(all(
    feature = "restart-announcements",
    feature = "pcp",
    feature = "nat-pmp"
))]
pub fn announcement(data: &[u8]) {
    crate::announce::fuzz::run(data);
}

/// SSDP search replies.
#[cfg(feature = "upnp")]
pub fn ssdp_reply(data: &[u8]) {
    crate::upnp::ssdp::fuzz::run(data);
}

/// HTTP responses from a UPnP gateway.
#[cfg(feature = "upnp")]
pub fn http_response(data: &[u8]) {
    crate::upnp::http::fuzz::run(data);
}

/// UPnP device descriptions.
#[cfg(feature = "upnp")]
pub fn device_description(data: &[u8]) {
    crate::upnp::device::fuzz::run(data);
}

/// SOAP responses and faults, and SOAP request arguments.
#[cfg(feature = "upnp")]
pub fn soap(data: &[u8]) {
    crate::upnp::soap::fuzz::run(data);
}

/// URLs from SSDP replies and device descriptions.
#[cfg(feature = "upnp")]
pub fn url(data: &[u8]) {
    crate::upnp::url::fuzz::run(data);
}

/// The Linux routing table, `/proc/net/route`.
#[cfg(target_os = "linux")]
pub fn proc_route(data: &[u8]) {
    crate::gateway::linux::fuzz::run(data);
}
