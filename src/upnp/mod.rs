//! UPnP IGD port mapping: find the gateway with SSDP, read its device
//! description, then call AddPortMapping on its WAN connection service.

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

use tracing::debug;

use crate::{
    Protocol,
    error::{Failure, Result},
    random, udp,
};

pub(crate) mod device;
pub(crate) mod http;
pub(crate) mod soap;
pub(crate) mod ssdp;
pub(crate) mod url;

use device::Service;
use url::Url;

/// Gateway error codes for AddPortMapping.
const CONFLICT_IN_MAPPING_ENTRY: u16 = 718;
const ONLY_PERMANENT_LEASES_SUPPORTED: u16 = 725;
/// How many other external ports to try when the port is taken.
const CONFLICT_TRIES: usize = 3;

/// Where to search for gateways.
#[derive(Clone, Debug)]
pub(crate) struct Search {
    /// The SSDP multicast group, or a test responder.
    pub(crate) dest: SocketAddrV4,
    /// A reply from the default gateway ends the search early.
    pub(crate) gateway: Option<Ipv4Addr>,
    pub(crate) wait: Duration,
}

/// A mapping granted by a UPnP gateway.
#[derive(Clone, Debug)]
pub(crate) struct Lease {
    service: Service,
    local_ip: Ipv4Addr,
    protocol: Protocol,
    local_port: u16,
    pub(crate) external: SocketAddrV4,
    /// The lease we asked for. 0 means permanent: the gateway grants no
    /// other kind.
    lease_secs: u32,
}

/// Finds a gateway and asks it for a new mapping of `local_port`.
pub(crate) async fn map(
    search: &Search,
    protocol: Protocol,
    local_port: u16,
    lifetime: u32,
    description: &str,
) -> Result<Lease> {
    let mut last = Failure::NoUpnpGateway;
    for location in ssdp::search(search.dest, search.gateway, search.wait).await? {
        match map_with(&location, protocol, local_port, lifetime, description).await {
            Ok(lease) => return Ok(lease),
            Err(e) => {
                debug!(%location, "UPnP gateway: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

/// Renews `lease`: the same mapping again, and the external address again.
pub(crate) async fn renew(lease: &Lease, description: &str) -> Result<Lease> {
    if udp::local_ip(lease.service.control.addr).await? != lease.local_ip {
        return Err(Failure::NetworkChanged);
    }
    let port = lease.external.port();
    add(lease, port, description).await?;
    let ip = external_ip(&lease.service).await?;
    Ok(Lease {
        external: SocketAddrV4::new(ip, port),
        ..lease.clone()
    })
}

/// Whether to delete `lease` when the task stops renewing it after
/// `failure`. A gateway keeps a permanent lease until it is deleted. After a
/// conflict, the external port is another host's mapping, so it is kept.
pub(crate) fn delete_on_loss(failure: &Failure) -> bool {
    !matches!(
        failure,
        Failure::Refused {
            code: CONFLICT_IN_MAPPING_ENTRY,
            ..
        }
    )
}

pub(crate) async fn release(lease: &Lease) -> Result<()> {
    let port = lease.external.port().to_string();
    let protocol = lease.protocol.to_string();
    let args = [
        ("NewRemoteHost", ""),
        ("NewExternalPort", port.as_str()),
        ("NewProtocol", protocol.as_str()),
    ];
    soap::call(&lease.service, "DeletePortMapping", &args).await?;
    Ok(())
}

/// Tries each WAN connection service that the description at `location`
/// lists.
async fn map_with(
    location: &Url,
    protocol: Protocol,
    local_port: u16,
    lifetime: u32,
    description: &str,
) -> Result<Lease> {
    let response = http::get(location).await?;
    if response.status != 200 {
        return Err(Failure::HttpStatus(response.status));
    }
    let services = device::services(&String::from_utf8_lossy(&response.body), location)?;
    let mut last = Failure::BadReply("no WAN connection service");
    for service in services {
        let local_ip = udp::local_ip(service.control.addr).await?;
        let mut lease = Lease {
            service,
            local_ip,
            protocol,
            local_port,
            external: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, local_port),
            lease_secs: lifetime,
        };
        match map_on(&mut lease, description).await {
            Ok(()) => return Ok(lease),
            Err(e) => {
                debug!(control = %lease.service.control, "UPnP service: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

/// Gets the external address, then adds the mapping. Fills in `lease`.
async fn map_on(lease: &mut Lease, description: &str) -> Result<()> {
    // A gateway may list a WAN connection that is down; it has no address.
    let ip = external_ip(&lease.service).await?;
    let mut port = lease.local_port;
    let mut conflicts = 0;
    loop {
        match add(lease, port, description).await {
            Ok(()) => break,
            Err(Failure::Refused {
                code: ONLY_PERMANENT_LEASES_SUPPORTED,
                ..
            }) if lease.lease_secs != 0 => {
                lease.lease_secs = 0;
            }
            // Another host has this external port.
            Err(Failure::Refused {
                code: CONFLICT_IN_MAPPING_ENTRY,
                ..
            }) if conflicts < CONFLICT_TRIES => {
                conflicts += 1;
                port = random_port()?;
            }
            Err(e) => return Err(e),
        }
    }
    lease.external = SocketAddrV4::new(ip, port);
    Ok(())
}

async fn add(lease: &Lease, external_port: u16, description: &str) -> Result<()> {
    let external_port = external_port.to_string();
    let protocol = lease.protocol.to_string();
    let local_port = lease.local_port.to_string();
    let local_ip = lease.local_ip.to_string();
    let lease_secs = lease.lease_secs.to_string();
    let args = [
        ("NewRemoteHost", ""),
        ("NewExternalPort", external_port.as_str()),
        ("NewProtocol", protocol.as_str()),
        ("NewInternalPort", local_port.as_str()),
        ("NewInternalClient", local_ip.as_str()),
        ("NewEnabled", "1"),
        ("NewPortMappingDescription", description),
        ("NewLeaseDuration", lease_secs.as_str()),
    ];
    soap::call(&lease.service, "AddPortMapping", &args).await?;
    Ok(())
}

async fn external_ip(service: &Service) -> Result<Ipv4Addr> {
    let output = soap::call(service, "GetExternalIPAddress", &[]).await?;
    let ip: Ipv4Addr = soap::arg(&output, "NewExternalIPAddress")
        .and_then(|ip| ip.parse().ok())
        .ok_or(Failure::BadReply("no external address"))?;
    if ip.is_unspecified() {
        return Err(Failure::BadReply("the gateway has no external address"));
    }
    Ok(ip)
}

/// A random port from 1024 to 65535.
fn random_port() -> Result<u16> {
    let [a, b] = random()?;
    Ok(1024 + u16::from_be_bytes([a, b]) % (u16::MAX - 1023))
}

#[cfg(test)]
mod tests;
