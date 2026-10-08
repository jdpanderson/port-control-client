//! A router port mapping client for Tokio.
//!
//! `port-control-client` asks the router to forward a port to this host,
//! renews the mapping, and removes it when you stop. It speaks PCP (RFC
//! 6887), UPnP IGD (versions 1 and 2) and NAT-PMP (RFC 6886). By default it
//! tries PCP, then UPnP; NAT-PMP is off. [`Config::methods`] changes the
//! protocols and their order.
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

#[cfg(not(any(feature = "pcp", feature = "nat-pmp", feature = "upnp")))]
compile_error!("port-control-client needs at least one of the features pcp, nat-pmp and upnp");

#[cfg(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
))]
mod announce;
mod error;
mod gateway;
mod mapper;
#[cfg(feature = "nat-pmp")]
mod nat_pmp;
#[cfg(feature = "pcp")]
mod pcp;
mod udp;
#[cfg(feature = "upnp")]
mod upnp;

// Entry points for the fuzz targets in `fuzz/`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzz;

#[cfg(test)]
mod responder;
// The tests of the whole task need every protocol.
#[cfg(all(test, feature = "pcp", feature = "nat-pmp", feature = "upnp"))]
mod fake;
#[cfg(all(test, feature = "pcp", feature = "nat-pmp", feature = "upnp"))]
mod tests;

// The README's example is compiled and checked as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use error::{Cause, Error, ErrorKind};
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

/// A port mapping protocol. Every variant exists in every build, but a
/// protocol works only when its feature is on: `pcp`, `nat-pmp` or `upnp`.
/// [`Config::methods`] can list one that is not built; trying it fails with
/// [`ErrorKind::NotBuilt`].
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

/// What the task has: a mapping or not, and why the last request failed.
/// See [`PortMapping::status`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// No mapping. `error` says why the last attempt failed: to get the
    /// mapping, or to renew one that then expired. `None` before the first
    /// attempt ends.
    Unmapped {
        /// Why the last attempt failed.
        error: Option<Error>,
    },
    /// The router granted `mapping`. `error` says why the last renewal
    /// failed, if it did; the mapping stays until it expires.
    Mapped {
        /// The granted mapping.
        mapping: Mapping,
        /// Why the last renewal failed.
        error: Option<Error>,
    },
    /// The task has stopped. `error` says why the release failed, or, with
    /// no mapping to release, why the last attempt failed.
    Stopped {
        /// Why the release, or the last attempt, failed.
        error: Option<Error>,
    },
}

impl Status {
    /// The granted mapping, if any.
    #[must_use]
    pub fn mapping(&self) -> Option<Mapping> {
        match self {
            Status::Mapped { mapping, .. } => Some(*mapping),
            Status::Unmapped { .. } | Status::Stopped { .. } => None,
        }
    }

    /// Why the last request failed, if it did.
    #[must_use]
    pub fn error(&self) -> Option<&Error> {
        match self {
            Status::Unmapped { error }
            | Status::Mapped { error, .. }
            | Status::Stopped { error } => error.as_ref(),
        }
    }
}

/// What to map, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    protocol: Protocol,
    local_port: NonZeroU16,
    lifetime: Duration,
    retry_interval: Duration,
    description: String,
    methods: Vec<Method>,
    #[cfg(feature = "restart-announcements")]
    restart_announcements: bool,
}

impl Config {
    /// The protocols to try, in order, by default: PCP, then UPnP, of those
    /// that are built. PCP replaces UPnP, but few routers have it. NAT-PMP
    /// is off, because few routers have it; add it with
    /// [`methods`](Self::methods).
    pub const DEFAULT_METHODS: &'static [Method] = &[
        #[cfg(feature = "pcp")]
        Method::Pcp,
        #[cfg(feature = "upnp")]
        Method::Upnp,
    ];

    /// Maps `local_port` for `protocol` with the protocols of
    /// [`DEFAULT_METHODS`](Self::DEFAULT_METHODS). The lifetime is two
    /// hours, and a failed attempt is retried after one minute.
    #[must_use]
    pub fn new(protocol: Protocol, local_port: NonZeroU16) -> Self {
        Self {
            protocol,
            local_port,
            lifetime: Duration::from_secs(2 * 60 * 60),
            retry_interval: Duration::from_secs(60),
            description: "port-control-client".to_owned(),
            methods: Self::DEFAULT_METHODS.to_vec(),
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

    /// The protocols to try, in this order. Protocols not in the list are
    /// off, and a protocol listed twice counts once, at its first place. For
    /// example, `[Method::Upnp, Method::Pcp]` tries UPnP first, then PCP,
    /// and never NAT-PMP. An empty list turns every protocol off.
    ///
    /// A held mapping is renewed with the protocol that granted it, whatever
    /// the order.
    #[must_use]
    pub fn methods(mut self, methods: impl IntoIterator<Item = Method>) -> Self {
        self.methods.clear();
        for method in methods {
            if !self.methods.contains(&method) {
                self.methods.push(method);
            }
        }
        self
    }

    /// Whether to listen for restart announcements from PCP and NAT-PMP
    /// routers. After a restart, the router has lost the mapping, and the
    /// task renews it within a few seconds, not at the next renewal. The
    /// task listens on UDP port 5350, which other programs on the host can
    /// share. Off by default. Needs the `restart-announcements` feature,
    /// and does nothing without the `pcp` and `nat-pmp` features.
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
    local_port: NonZeroU16,
    /// Changes after `status`, so a user that sees a new mapping here also
    /// sees it in `status`.
    state: watch::Receiver<Option<Mapping>>,
    status: watch::Receiver<Status>,
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
        let local_port = config.local_port;
        let (state_sender, state) = watch::channel(None);
        let (status_sender, status) = watch::channel(Status::Unmapped { error: None });
        let report = mapper::Report {
            state: state_sender,
            status: status_sender,
        };
        tokio::spawn(mapper::run(config, targets, receiver, report));
        Self {
            commands,
            local_port,
            state,
            status,
        }
    }

    /// The local port that the mapping is for.
    #[must_use]
    pub fn local_port(&self) -> NonZeroU16 {
        self.local_port
    }

    /// The mapping and the last error, read together, so they always
    /// agree.
    #[must_use]
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// The mapping the router has granted, if any.
    #[must_use]
    pub fn mapping(&self) -> Option<Mapping> {
        self.status.borrow().mapping()
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
        self.status.borrow().error().cloned()
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
#[cfg(feature = "pcp")]
fn random_fraction() -> f64 {
    random::<1>().map_or(0.5, |[b]| f64::from(b) / 255.0)
}

/// `N` random bytes from the operating system.
#[cfg(any(feature = "pcp", feature = "upnp"))]
fn random<const N: usize>() -> error::Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|e| error::Failure::Io(std::io::Error::other(e)))?;
    Ok(bytes)
}
