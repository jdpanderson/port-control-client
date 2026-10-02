//! A router port mapping client for Tokio.
//!
//! `port-control-client` asks the router to forward a port to this host,
//! renews the mapping, and removes it when you stop. It tries PCP (RFC
//! 6887), then NAT-PMP (RFC 6886), then UPnP IGD (versions 1 and 2).
//!
//! IPv4 only. PCP and NAT-PMP need the default gateway, which the crate finds
//! only on Linux and macOS. Other systems use only UPnP.
//!
//! ```no_run
//! use std::{num::NonZeroU16, time::Duration};
//! use port_control_client::{Config, PortMapping, Protocol};
//!
//! # async fn example() {
//! let port = NonZeroU16::new(51820).unwrap();
//! let mapping = PortMapping::start(Config::new(Protocol::Udp, port));
//!
//! // Wait up to 10 seconds for the router to grant the mapping.
//! let mut watch = mapping.watch();
//! let _ = tokio::time::timeout(Duration::from_secs(10), watch.wait_for(Option::is_some)).await;
//! if let Some(m) = mapping.mapping() {
//!     println!("reachable at {} through {}", m.external, m.method);
//! } else if let Some(error) = mapping.last_error() {
//!     println!("no mapping: {error}");
//! }
//!
//! // Remove the mapping from the router.
//! mapping.stop().await;
//! # }
//! ```
//!
//! [`PortMapping::last_error`] has a [`Cause`] for each protocol tried.
//!
//! Behind a second NAT, the router's external address is private, and the
//! port is not reachable from the internet.

use std::{fmt, net::SocketAddrV4, num::NonZeroU16, time::Duration};

use tokio::sync::{mpsc, oneshot, watch};

#[cfg(feature = "restart-announcements")]
mod announce;
mod error;
mod gateway;
mod mapper;
mod nat_pmp;
mod pcp;
mod udp;
mod upnp;

// Entry points for the fuzz targets in `fuzz/`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzz;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

// The README's example is compiled and checked as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use error::{Cause, Error, ErrorKind};
use error::{Failure, Result};
use mapper::{Command, Targets};

/// The transport protocol of the port to map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// UDP.
    Udp,
    /// TCP.
    Tcp,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Protocol::Udp => "UDP",
            Protocol::Tcp => "TCP",
        })
    }
}

/// The protocol a router granted a mapping with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Method {
    /// The Port Control Protocol (RFC 6887).
    Pcp,
    /// NAT-PMP (RFC 6886).
    NatPmp,
    /// UPnP Internet Gateway Device.
    Upnp,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Method::Pcp => "PCP",
            Method::NatPmp => "NAT-PMP",
            Method::Upnp => "UPnP",
        })
    }
}

/// A mapping the router has granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Mapping {
    /// The router's external address and port. Traffic to it goes to the
    /// local port.
    pub external: SocketAddrV4,
    /// The protocol the router granted the mapping with.
    pub method: Method,
}

/// What to map, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    protocol: Protocol,
    local_port: NonZeroU16,
    lifetime: Duration,
    retry_interval: Duration,
    description: String,
    pcp: bool,
    nat_pmp: bool,
    upnp: bool,
    #[cfg(feature = "restart-announcements")]
    restart_announcements: bool,
}

impl Config {
    /// Maps `local_port` for `protocol`. All three protocols are on, the
    /// lifetime is two hours, and a failed attempt is retried after one
    /// minute.
    #[must_use]
    pub fn new(protocol: Protocol, local_port: NonZeroU16) -> Self {
        Self {
            protocol,
            local_port,
            lifetime: Duration::from_secs(2 * 60 * 60),
            retry_interval: Duration::from_secs(60),
            description: "port-control-client".to_owned(),
            pcp: true,
            nat_pmp: true,
            upnp: true,
            #[cfg(feature = "restart-announcements")]
            restart_announcements: false,
        }
    }

    /// The lifetime to ask the router for. The router may grant less. The
    /// mapping is renewed at half the granted lifetime. Values count as at
    /// least one second, and at most `u32::MAX` seconds.
    ///
    /// Use one week or less. Some UPnP gateways cut longer leases to one
    /// week and do not say so, because UPnP does not report the granted
    /// lifetime. The mapping then expires before the task renews it.
    #[must_use]
    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// How long to wait before asking again when no router grants a
    /// mapping. Values count as at least one second, and at most
    /// `u32::MAX` seconds.
    #[must_use]
    pub fn retry_interval(mut self, interval: Duration) -> Self {
        self.retry_interval = interval;
        self
    }

    /// The description that UPnP routers show for the mapping. Characters
    /// that XML cannot contain, such as most control characters, are left
    /// out.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Whether to try PCP.
    #[must_use]
    pub fn pcp(mut self, on: bool) -> Self {
        self.pcp = on;
        self
    }

    /// Whether to try NAT-PMP.
    #[must_use]
    pub fn nat_pmp(mut self, on: bool) -> Self {
        self.nat_pmp = on;
        self
    }

    /// Whether to try UPnP.
    #[must_use]
    pub fn upnp(mut self, on: bool) -> Self {
        self.upnp = on;
        self
    }

    /// Whether to listen for restart announcements from PCP and NAT-PMP
    /// routers. After a restart, the router has lost the mapping, and the
    /// task renews it within a few seconds, not at the next renewal. The
    /// task listens on UDP port 5350, which other programs on the host can
    /// share. Off by default. Needs the `restart-announcements` feature.
    #[cfg(feature = "restart-announcements")]
    #[must_use]
    pub fn restart_announcements(mut self, on: bool) -> Self {
        self.restart_announcements = on;
        self
    }

    /// The lifetime in whole seconds, from 1 to `u32::MAX`. Zero would ask
    /// the router to delete the mapping (PCP, NAT-PMP) or to make it
    /// permanent (UPnP).
    fn lifetime_secs(&self) -> u32 {
        u32::try_from(self.lifetime.as_secs())
            .unwrap_or(u32::MAX)
            .max(1)
    }

    /// The retry interval, at most `u32::MAX` seconds so that a deadline
    /// cannot overflow. The task waits at least one second.
    fn retry_wait(&self) -> Duration {
        self.retry_interval
            .min(Duration::from_secs(u32::MAX.into()))
    }
}

/// A port mapping that a background task gets, renews and releases.
///
/// [`start`](Self::start) returns immediately. The task tries each protocol
/// in order, and tries again after the retry interval if none works. After
/// the router grants the mapping, the task renews it at half its lifetime.
///
/// Dropping the handle releases the mapping in the background. Use
/// [`stop`](Self::stop) to wait for the release.
#[derive(Debug)]
#[must_use = "dropping the handle releases the mapping"]
pub struct PortMapping {
    commands: mpsc::Sender<Command>,
    state: watch::Receiver<Option<Mapping>>,
    errors: watch::Receiver<Option<Error>>,
}

impl PortMapping {
    /// Starts the task on the current Tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime.
    pub fn start(config: Config) -> Self {
        Self::start_with(config, Targets::system())
    }

    fn start_with(config: Config, targets: Targets) -> Self {
        let (commands, receiver) = mpsc::channel(4);
        let (sender, state) = watch::channel(None);
        let (error_sender, errors) = watch::channel(None);
        tokio::spawn(mapper::run(config, targets, receiver, sender, error_sender));
        Self {
            commands,
            state,
            errors,
        }
    }

    /// The mapping the router has granted, if any.
    #[must_use]
    pub fn mapping(&self) -> Option<Mapping> {
        *self.state.borrow()
    }

    /// A receiver for [`mapping`](Self::mapping). It always has the newest
    /// value, but it can miss values that change between two reads.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Option<Mapping>> {
        self.state.clone()
    }

    /// Why the last attempt failed: to get the mapping, or to renew it.
    /// `None` once an attempt works. After [`stop`](Self::stop) releases a
    /// mapping, it is the release's result. The task also logs failures with
    /// `tracing`: at the `info` level when it loses a mapping, and at the
    /// `debug` level otherwise.
    #[must_use]
    pub fn last_error(&self) -> Option<Error> {
        self.errors.borrow().clone()
    }

    /// Asks the router again now: renews the mapping, or tries to get one.
    /// Use it after the network changes.
    pub fn refresh(&self) {
        // A full queue already holds a request, so this one is not needed.
        let _ = self.commands.try_send(Command::Refresh);
    }

    /// Releases the mapping on the router and stops the task. Returns when
    /// the router answers, or after two seconds.
    pub async fn stop(&self) {
        let (done, wait) = oneshot::channel();
        if self.commands.send(Command::Stop(done)).await.is_ok() {
            // An error means the task has already stopped.
            let _ = wait.await;
        }
    }
}

/// A random number from 0 to 1. Without the operating system's random
/// source, 0.5.
fn random_fraction() -> f64 {
    random::<1>().map_or(0.5, |[b]| f64::from(b) / 255.0)
}

/// `N` random bytes from the operating system.
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|e| Failure::Io(std::io::Error::other(e)))?;
    Ok(bytes)
}
