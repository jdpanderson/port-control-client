//! The task behind a [`PortMapping`](crate::PortMapping): it gets a
//! mapping, renews it, and releases it.

use std::net::{Ipv4Addr, SocketAddrV4};

use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Duration, Instant, sleep_until, timeout},
};
use tracing::{debug, info};

#[cfg(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
))]
use crate::announce::{self, Epoch, Kind, Listener};
#[cfg(feature = "nat-pmp")]
use crate::nat_pmp;
#[cfg(all(feature = "restart-announcements", feature = "pcp"))]
use crate::random_fraction;
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
use crate::udp;
#[cfg(feature = "upnp")]
use crate::upnp::{self, ssdp};
use crate::{
    Config, Mapping, Method, Status,
    error::{Error, Failure, Result},
    gateway,
};
#[cfg(feature = "pcp")]
use crate::{pcp, random};

/// The shortest wait between two steps, so that a router that grants very
/// short lifetimes does not keep the task busy.
const MIN_WAIT: Duration = Duration::from_secs(1);
/// How long `stop` waits for the router to release the mapping.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for SSDP replies.
#[cfg(feature = "upnp")]
const SSDP_WAIT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum Command {
    Refresh,
    Stop(oneshot::Sender<()>),
}

/// Where requests go. Tests point these at fake routers.
#[derive(Clone, Debug)]
pub(crate) struct Targets {
    pub(crate) gateway: Gateway,
    /// The PCP and NAT-PMP server port.
    #[cfg(any(feature = "pcp", feature = "nat-pmp"))]
    pub(crate) pmp_port: u16,
    #[cfg(feature = "upnp")]
    pub(crate) ssdp: SocketAddrV4,
    #[cfg(feature = "upnp")]
    pub(crate) ssdp_wait: Duration,
    /// How long PCP waits for a reply after each send.
    #[cfg(feature = "pcp")]
    pub(crate) pcp_wait: Duration,
    #[cfg(all(
        feature = "restart-announcements",
        any(feature = "pcp", feature = "nat-pmp")
    ))]
    pub(crate) announce: announce::Targets,
}

/// The default gateway, for PCP and NAT-PMP.
#[derive(Clone, Debug)]
pub(crate) enum Gateway {
    /// Look up the system's default gateway each time.
    System,
    #[cfg(all(test, any(feature = "pcp", feature = "nat-pmp")))]
    Fixed(Ipv4Addr),
    /// A gateway that a test can change.
    #[cfg(all(test, feature = "pcp", feature = "nat-pmp", feature = "upnp"))]
    Changing(std::sync::Arc<std::sync::atomic::AtomicU32>),
    /// A system with no default gateway.
    #[cfg(all(test, feature = "pcp", feature = "nat-pmp", feature = "upnp"))]
    Missing,
}

impl Targets {
    pub(crate) fn system() -> Self {
        Self {
            gateway: Gateway::System,
            #[cfg(any(feature = "pcp", feature = "nat-pmp"))]
            pmp_port: udp::PMP_PORT,
            #[cfg(feature = "upnp")]
            ssdp: ssdp::MULTICAST,
            #[cfg(feature = "upnp")]
            ssdp_wait: SSDP_WAIT,
            #[cfg(feature = "pcp")]
            pcp_wait: pcp::WAIT,
            #[cfg(all(
                feature = "restart-announcements",
                any(feature = "pcp", feature = "nat-pmp")
            ))]
            announce: announce::Targets::system(),
        }
    }

    fn gateway(&self) -> Result<Ipv4Addr> {
        match &self.gateway {
            Gateway::System => gateway::default_gateway().map_err(Failure::NoDefaultGateway),
            #[cfg(all(test, any(feature = "pcp", feature = "nat-pmp")))]
            Gateway::Fixed(gateway) => Ok(*gateway),
            #[cfg(all(test, feature = "pcp", feature = "nat-pmp", feature = "upnp"))]
            Gateway::Changing(gateway) => {
                Ok(gateway.load(std::sync::atomic::Ordering::SeqCst).into())
            }
            #[cfg(all(test, feature = "pcp", feature = "nat-pmp", feature = "upnp"))]
            Gateway::Missing => Err(Failure::NoDefaultGateway(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no IPv4 default route",
            ))),
        }
    }
}

#[derive(Clone, Debug)]
enum Lease {
    #[cfg(feature = "pcp")]
    Pcp(pcp::Lease),
    #[cfg(feature = "nat-pmp")]
    NatPmp(nat_pmp::Lease),
    #[cfg(feature = "upnp")]
    Upnp(upnp::Lease),
}

impl Lease {
    fn mapping(&self) -> Mapping {
        let (external, method) = match self {
            #[cfg(feature = "pcp")]
            Lease::Pcp(l) => (l.external, Method::Pcp),
            #[cfg(feature = "nat-pmp")]
            Lease::NatPmp(l) => (l.external, Method::NatPmp),
            #[cfg(feature = "upnp")]
            Lease::Upnp(l) => (l.external, Method::Upnp),
        };
        Mapping { external, method }
    }

    /// For PCP and NAT-PMP leases: the server, our address, the kind of
    /// restart announcements, and the server's epoch.
    #[cfg(all(
        feature = "restart-announcements",
        any(feature = "pcp", feature = "nat-pmp")
    ))]
    fn announcer(&mut self) -> Option<(SocketAddrV4, Ipv4Addr, Kind, &mut Epoch)> {
        match self {
            #[cfg(feature = "pcp")]
            Lease::Pcp(l) => Some((l.server, l.local_ip, Kind::Pcp, &mut l.epoch)),
            #[cfg(feature = "nat-pmp")]
            Lease::NatPmp(l) => Some((l.server, l.local_ip, Kind::NatPmp, &mut l.epoch)),
            #[cfg(feature = "upnp")]
            Lease::Upnp(_) => None,
        }
    }

    /// The granted lifetime. `None` for UPnP: gateways do not say what
    /// they grant.
    fn lifetime(&self) -> Option<Duration> {
        match self {
            #[cfg(feature = "pcp")]
            Lease::Pcp(l) => Some(l.lifetime),
            #[cfg(feature = "nat-pmp")]
            Lease::NatPmp(l) => Some(l.lifetime),
            #[cfg(feature = "upnp")]
            Lease::Upnp(_) => None,
        }
    }

    #[cfg(feature = "upnp")]
    fn upnp(&self) -> Option<&upnp::Lease> {
        match self {
            Lease::Upnp(l) => Some(l),
            #[cfg(any(feature = "pcp", feature = "nat-pmp"))]
            _ => None,
        }
    }
}

/// A lease, and when it ends if it is not renewed.
#[derive(Clone, Debug)]
struct Held {
    lease: Lease,
    expires: Instant,
}

/// Where the task shows what it has.
#[derive(Debug)]
pub(crate) struct Report {
    /// The current mapping, for `PortMapping::watch`.
    pub(crate) state: watch::Sender<Option<Mapping>>,
    /// The mapping and the last error together.
    pub(crate) status: watch::Sender<Status>,
}

impl Report {
    fn shown(&self) -> bool {
        self.state.borrow().is_some()
    }

    /// Shows `status`. `state` changes last, so a user that sees a new
    /// mapping in `state` also sees it in `status`.
    fn show(&self, status: Status) {
        let mapping = status.mapping();
        self.status.send_replace(status);
        self.state.send_if_modified(|current| {
            let changed = *current != mapping;
            *current = mapping;
            changed
        });
    }

    /// The held mapping has expired. The last error stays.
    fn expire(&self) {
        let error = self.status.borrow().error().cloned();
        self.show(Status::Unmapped { error });
    }
}

/// Runs until a `Stop` command, or until every handle is dropped. Each
/// step gets a mapping or renews one, and `report` shows the result.
pub(crate) async fn run(
    config: Config,
    targets: Targets,
    mut commands: mpsc::Receiver<Command>,
    report: Report,
) {
    let mut held: Option<Held> = None;
    let mut nonce = Nonce::default();
    let mut next = Instant::now();
    // Announcements do not bring steps closer together than `MIN_WAIT`.
    let mut last_step = Instant::now();
    let mut restarts = Restarts::new(&config);
    loop {
        // The wait for the next step, or the step itself, can last past the
        // expiry: stop showing the mapping then.
        let expires = held.as_ref().map(|h| h.expires);
        tokio::select! {
            () = sleep_until(next) => {}
            () = sleep_until(expires.unwrap_or_else(Instant::now)), if expires.is_some() && report.shown() => {
                report.expire();
                continue;
            }
            delay = restarts.next(held.as_mut(), &targets) => {
                info!("the router restarted: renewing the port mapping");
                next = next.min((Instant::now() + delay).max(last_step + MIN_WAIT));
                continue;
            }
            command = commands.recv() => if let Some(stop) = Stop::from(command) {
                return finish(held, &report, stop).await;
            },
        }
        // A stop during a step drops the step. A mapping that a dropped
        // step was getting stays on the router until its lifetime ends.
        // A refresh during a step may come from a network change that the
        // step did not see: run the next step now.
        let mut refreshed = false;
        let outcome = {
            let work = step(&config, &targets, held.as_ref(), &mut nonce);
            tokio::pin!(work);
            loop {
                tokio::select! {
                    outcome = &mut work => break Ok(outcome),
                    () = sleep_until(expires.unwrap_or_else(Instant::now)), if expires.is_some() && report.shown() => {
                        report.expire();
                    }
                    command = commands.recv() => match Stop::from(command) {
                        Some(stop) => break Err(stop),
                        None => refreshed = true,
                    },
                }
            }
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(stop) => return finish(held, &report, stop).await,
        };
        held = outcome.held;
        last_step = Instant::now();
        restarts.update(&targets, held.as_mut());
        next = Instant::now()
            + if refreshed {
                Duration::ZERO
            } else {
                outcome.wait.max(MIN_WAIT)
            };
        let error = outcome.error;
        report.show(match &held {
            Some(h) => Status::Mapped {
                mapping: h.lease.mapping(),
                error,
            },
            None => Status::Unmapped { error },
        });
    }
}

/// Listens for restart announcements while the task holds a PCP or NAT-PMP
/// lease, if the config turns this on.
#[cfg(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
))]
#[derive(Debug)]
struct Restarts {
    on: bool,
    listener: Option<Listener>,
}

#[cfg(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
))]
impl Restarts {
    fn new(config: &Config) -> Self {
        Self {
            on: config.restart_announcements,
            listener: None,
        }
    }

    /// Opens the listener for a PCP or NAT-PMP lease, and closes it without
    /// one. Without a listener, a mapping that the router lost comes back
    /// at the next renewal.
    fn update(&mut self, targets: &Targets, held: Option<&mut Held>) {
        let announcer = held.and_then(|h| h.lease.announcer()).filter(|_| self.on);
        let Some((_, local_ip, _, _)) = announcer else {
            self.listener = None;
            return;
        };
        if self.listener.is_some() {
            return;
        }
        let at = &targets.announce;
        match Listener::bind(at.addr, at.group, local_ip) {
            Ok(listener) => self.listener = Some(listener),
            Err(e) => debug!("can't listen for router restarts: {e}"),
        }
    }

    /// Waits for an announcement that the server of `held` restarted, and
    /// returns how long to wait before the renewal. Never ends without a
    /// listener.
    async fn next(&mut self, held: Option<&mut Held>, targets: &Targets) -> Duration {
        let announcer = held.and_then(|h| h.lease.announcer());
        let (Some(listener), Some((server, _, kind, epoch))) = (&self.listener, announcer) else {
            return std::future::pending().await;
        };
        match restart_delay(listener, server, kind, epoch, targets.announce.delay).await {
            Ok(delay) => delay,
            Err(e) => {
                debug!("can't listen for router restarts: {e}");
                self.listener = None;
                std::future::pending().await
            }
        }
    }
}

/// Without the `restart-announcements` feature, the task does not listen
/// for restarts.
#[cfg(not(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
)))]
#[derive(Debug)]
struct Restarts;

#[cfg(not(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
)))]
impl Restarts {
    fn new(_: &Config) -> Self {
        Self
    }

    fn update(&mut self, _: &Targets, _: Option<&mut Held>) {}

    async fn next(&mut self, _: Option<&mut Held>, _: &Targets) -> Duration {
        std::future::pending().await
    }
}

/// Waits for an announcement from `server` that shows a restart, and
/// returns how long to wait before the renewal.
#[cfg(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
))]
async fn restart_delay(
    listener: &Listener,
    server: SocketAddrV4,
    kind: Kind,
    epoch: &mut Epoch,
    delay: Duration,
) -> std::io::Result<Duration> {
    loop {
        let next = listener.next(server, kind).await?;
        if let Some(wait) = restarted(epoch, kind, next, delay) {
            return Ok(wait);
        }
    }
}

/// Checks the epoch time `next` in a restart announcement against `epoch`,
/// and keeps it. After a restart, it returns how long to wait before the
/// renewal: PCP waits a random time up to `delay` (RFC 6887, section
/// 14.1.3), and NAT-PMP renews now (RFC 6886, section 3.2.1).
#[cfg(all(
    feature = "restart-announcements",
    any(feature = "pcp", feature = "nat-pmp")
))]
#[cfg_attr(
    not(feature = "pcp"),
    expect(unused_variables, reason = "the delay is for PCP")
)]
fn restarted(epoch: &mut Epoch, kind: Kind, next: u32, delay: Duration) -> Option<Duration> {
    let lost = epoch.lost(kind, next, Instant::now());
    *epoch = Epoch::new(next);
    if !lost {
        return None;
    }
    Some(match kind {
        #[cfg(feature = "pcp")]
        Kind::Pcp => delay.mul_f64(random_fraction()),
        #[cfg(feature = "nat-pmp")]
        Kind::NatPmp => Duration::ZERO,
    })
}

/// What a step leaves: the lease we hold, how long to wait before the next
/// step, and why the step failed, if it did.
struct Outcome {
    held: Option<Held>,
    wait: Duration,
    error: Option<Error>,
}

/// Gets a mapping, or renews the one in `held`.
async fn step(
    config: &Config,
    targets: &Targets,
    held: Option<&Held>,
    nonce: &mut Nonce,
) -> Outcome {
    let Some(old) = held else {
        return match acquire(config, targets, nonce).await {
            Ok(lease) => {
                let mapping = lease.mapping();
                info!(external = %mapping.external, method = %mapping.method, "got a port mapping");
                hold(config, lease)
            }
            Err(error) => {
                debug!("no port mapping: {error}");
                Outcome {
                    held: None,
                    wait: config.retry_wait(),
                    error: Some(error),
                }
            }
        };
    };
    let method = old.lease.mapping().method;
    let failure = match renew(config, targets, &old.lease).await {
        Ok(lease) => {
            let (before, after) = (old.lease.mapping(), lease.mapping());
            if before != after {
                info!(external = %after.external, method = %after.method, "port mapping changed");
            }
            return hold(config, lease);
        }
        Err(failure) => failure,
    };
    let error = Error::new(vec![failure.cause(Some(method))]);
    let left = old.expires.saturating_duration_since(Instant::now());
    // Ask for a new mapping now.
    if failure.mapping_gone() || left.is_zero() {
        info!("lost the port mapping: {error}");
        #[cfg(feature = "upnp")]
        if let Some(lease) = old.lease.upnp() {
            if upnp::delete_on_loss(&failure) {
                discard(lease).await;
            }
        }
        return Outcome {
            held: None,
            wait: Duration::ZERO,
            error: Some(error),
        };
    }
    // Try again before the mapping expires.
    debug!("can't renew the port mapping yet: {error}");
    Outcome {
        held: Some(old.clone()),
        wait: (left / 2).min(config.retry_wait()),
        error: Some(error),
    }
}

/// Deletes a UPnP lease that the task no longer renews, if the gateway
/// still answers.
#[cfg(feature = "upnp")]
async fn discard(lease: &upnp::Lease) {
    let result = timeout(RELEASE_TIMEOUT, upnp::release(lease))
        .await
        .unwrap_or(Err(Failure::Timeout));
    if let Err(failure) = result {
        debug!(external = %lease.external, "can't delete the lost port mapping: {failure}");
    }
}

fn hold(config: &Config, lease: Lease) -> Outcome {
    // For UPnP, the lifetime we asked for.
    let lifetime = lease
        .lifetime()
        .unwrap_or_else(|| Duration::from_secs(config.lifetime_secs().into()));
    let expires = Instant::now() + lifetime;
    Outcome {
        held: Some(Held { lease, expires }),
        wait: lifetime / 2,
        error: None,
    }
}

/// Tries each protocol of the config, in order. The error has a cause for
/// each one that failed.
#[cfg_attr(
    not(feature = "pcp"),
    expect(unused_variables, reason = "the nonce is for PCP")
)]
async fn acquire(config: &Config, targets: &Targets, nonce: &mut Nonce) -> Result<Lease, Error> {
    let mut causes = Vec::new();
    // Looked up when a protocol first needs it.
    let mut gateway = None;
    for &method in &config.methods {
        let result = match method {
            #[cfg(feature = "pcp")]
            Method::Pcp => {
                let Some(server) = pmp_server(targets, &mut gateway, &mut causes) else {
                    continue;
                };
                pcp_map(config, targets, server, nonce)
                    .await
                    .map(Lease::Pcp)
            }
            #[cfg(feature = "nat-pmp")]
            Method::NatPmp => {
                let Some(server) = pmp_server(targets, &mut gateway, &mut causes) else {
                    continue;
                };
                let (port, lifetime) = (config.local_port.get(), config.lifetime_secs());
                nat_pmp::map(server, config.protocol, port, lifetime)
                    .await
                    .map(Lease::NatPmp)
            }
            // UPnP finds its gateway by search; the default gateway only
            // ends the search early.
            #[cfg(feature = "upnp")]
            Method::Upnp => {
                let gateway = gateway.get_or_insert_with(|| targets.gateway());
                let search = upnp::Search {
                    dest: targets.ssdp,
                    gateway: gateway.as_ref().ok().copied(),
                    wait: targets.ssdp_wait,
                };
                let (port, lifetime) = (config.local_port.get(), config.lifetime_secs());
                let description = &config.description;
                upnp::map(&search, config.protocol, port, lifetime, description)
                    .await
                    .map(Lease::Upnp)
            }
            #[cfg(not(all(feature = "pcp", feature = "nat-pmp", feature = "upnp")))]
            _ => Err(Failure::NotBuilt),
        };
        match result {
            Ok(lease) => return Ok(lease),
            Err(e) => causes.push(e.cause(Some(method))),
        }
    }
    if causes.is_empty() {
        causes.push(Failure::NoProtocol.cause(None));
    }
    Err(Error::new(causes))
}

/// The PCP and NAT-PMP server on the default `gateway`, which it looks up
/// on first use. Without a default gateway, `None`. PCP and NAT-PMP then
/// share one cause in `causes`.
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
fn pmp_server(
    targets: &Targets,
    gateway: &mut Option<Result<Ipv4Addr>>,
    causes: &mut Vec<crate::Cause>,
) -> Option<SocketAddrV4> {
    match gateway.get_or_insert_with(|| targets.gateway()) {
        Ok(gateway) => Some(SocketAddrV4::new(*gateway, targets.pmp_port)),
        Err(e) => {
            let kind = crate::ErrorKind::NoDefaultGateway;
            if !causes.iter().any(|c| c.kind() == kind) {
                causes.push(e.cause(None));
            }
            None
        }
    }
}

/// Asks `server` for a PCP mapping, with the task's nonce.
#[cfg(feature = "pcp")]
async fn pcp_map(
    config: &Config,
    targets: &Targets,
    server: SocketAddrV4,
    nonce: &mut Nonce,
) -> Result<pcp::Lease> {
    let nonce = nonce.get()?;
    let (port, lifetime) = (config.local_port.get(), config.lifetime_secs());
    pcp::map(
        server,
        nonce,
        config.protocol,
        port,
        lifetime,
        targets.pcp_wait,
    )
    .await
}

/// The task's PCP nonce, made on first use. Every request of the task uses
/// it; see [`pcp::map`].
#[derive(Debug, Default)]
struct Nonce(#[cfg(feature = "pcp")] Option<pcp::Nonce>);

#[cfg(feature = "pcp")]
impl Nonce {
    fn get(&mut self) -> Result<pcp::Nonce> {
        if let Some(nonce) = self.0 {
            return Ok(nonce);
        }
        Ok(*self.0.insert(random()?))
    }
}

#[cfg_attr(
    not(any(feature = "pcp", feature = "nat-pmp")),
    expect(unused_variables, reason = "only PCP and NAT-PMP use the targets")
)]
async fn renew(config: &Config, targets: &Targets, lease: &Lease) -> Result<Lease> {
    match lease {
        #[cfg(feature = "pcp")]
        Lease::Pcp(l) => {
            same_gateway(targets, l.server)?;
            let lifetime = config.lifetime_secs();
            Ok(Lease::Pcp(pcp::renew(l, lifetime, targets.pcp_wait).await?))
        }
        #[cfg(feature = "nat-pmp")]
        Lease::NatPmp(l) => {
            same_gateway(targets, l.server)?;
            let lifetime = config.lifetime_secs();
            Ok(Lease::NatPmp(nat_pmp::renew(l, lifetime).await?))
        }
        #[cfg(feature = "upnp")]
        Lease::Upnp(l) => Ok(Lease::Upnp(upnp::renew(l, &config.description).await?)),
    }
}

/// A new default gateway means a new network: the old mapping is of no use.
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
fn same_gateway(targets: &Targets, server: SocketAddrV4) -> Result<()> {
    if targets.gateway()? == *server.ip() {
        Ok(())
    } else {
        Err(Failure::NetworkChanged)
    }
}

/// A request to stop: a `Stop` command, which waits for the release, or
/// the last handle dropped.
struct Stop(Option<oneshot::Sender<()>>);

impl Stop {
    /// `None` for a command that does not stop the task.
    fn from(command: Option<Command>) -> Option<Stop> {
        match command {
            Some(Command::Refresh) => None,
            Some(Command::Stop(done)) => Some(Stop(Some(done))),
            None => Some(Stop(None)),
        }
    }
}

async fn finish(held: Option<Held>, report: &Report, Stop(done): Stop) {
    let error = report.status.borrow().error().cloned();
    report.show(Status::Stopped { error });
    if let Some(held) = held {
        let mapping = held.lease.mapping();
        let release = async {
            match &held.lease {
                #[cfg(feature = "pcp")]
                Lease::Pcp(l) => pcp::release(l).await,
                #[cfg(feature = "nat-pmp")]
                Lease::NatPmp(l) => nat_pmp::release(l).await,
                #[cfg(feature = "upnp")]
                Lease::Upnp(l) => upnp::release(l).await,
            }
        };
        let result = timeout(RELEASE_TIMEOUT, release)
            .await
            .unwrap_or(Err(Failure::Timeout));
        match result {
            Ok(()) => {
                info!(external = %mapping.external, "released the port mapping");
                report.show(Status::Stopped { error: None });
            }
            Err(failure) => {
                let error = Error::new(vec![failure.cause(Some(mapping.method))]);
                debug!(external = %mapping.external, "can't release the port mapping: {error}");
                report.show(Status::Stopped { error: Some(error) });
            }
        }
    }
    if let Some(done) = done {
        let _ = done.send(());
    }
}

#[cfg(all(test, any(feature = "pcp", feature = "nat-pmp")))]
mod tests;
