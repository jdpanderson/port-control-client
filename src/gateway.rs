//! The IPv4 default gateway, where PCP and NAT-PMP requests go.

use std::io;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::net::Ipv4Addr;

#[cfg(any(target_os = "linux", test))]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub(crate) use linux::default_gateway;
#[cfg(target_os = "macos")]
pub(crate) use macos::default_gateway;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn default_gateway() -> io::Result<Ipv4Addr> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no default gateway lookup on this system",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn no_default_route() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no IPv4 default route")
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
mod tests;
