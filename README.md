# port-control-client

[![crates.io](https://img.shields.io/crates/v/port-control-client.svg)](https://crates.io/crates/port-control-client)
[![docs.rs](https://img.shields.io/docsrs/port-control-client)](https://docs.rs/port-control-client)
[![CI](https://github.com/jdpanderson/port-control-client/actions/workflows/ci.yml/badge.svg)](https://github.com/jdpanderson/port-control-client/actions/workflows/ci.yml)
[![MSRV 1.85](https://img.shields.io/badge/MSRV-1.85-blue.svg)](Cargo.toml)

A router port mapping client for [Tokio](https://tokio.rs). It asks the
router to forward a port to this host, renews the mapping, and removes it
when you stop. It speaks PCP ([RFC 6887]), UPnP IGD (versions 1 and 2)
and NAT-PMP ([RFC 6886]). By default it tries PCP, then UPnP. NAT-PMP is
off by default, because few routers have it. To change the order, or to
choose the protocols, use `Config::methods`:

```rust,no_run
# use std::num::NonZeroU16;
# use port_control_client::{Config, Method, Protocol};
# let port = NonZeroU16::new(51820).unwrap();
let config = Config::new(Protocol::Udp, port)
    .methods([Method::Upnp, Method::Pcp, Method::NatPmp]);
```

```rust,no_run
use std::{num::NonZeroU16, time::Duration};
use port_control_client::{Config, PortMapping, Protocol};

#[tokio::main]
async fn main() {
    let port = NonZeroU16::new(51820).unwrap();
    let mapping = PortMapping::start(Config::new(Protocol::Udp, port));

    // Wait up to 10 seconds for the router to grant the mapping.
    let mut watch = mapping.watch();
    let _ = tokio::time::timeout(Duration::from_secs(10), watch.wait_for(Option::is_some)).await;
    if let Some(m) = mapping.mapping() {
        println!("reachable at {} through {}", m.external, m.method);
    } else if let Some(error) = mapping.last_error() {
        println!("no mapping: {error}");
    }

    // Remove the mapping from the router.
    mapping.stop().await;
}
```

To test it on your network: `cargo run --example map -- udp 51820`.

## Behavior

- A background task gets the mapping and renews it at half its lifetime.
  If no router grants a mapping, the task tries again after the retry
  interval (default: one minute). `refresh()` tries again now.
- PCP sends twice, 3 seconds apart, NAT-PMP three times in 1.75 seconds,
  and UPnP searches for 2 seconds. Then the task tries the next protocol.
- Optional: with the `restart-announcements` feature and
  `Config::restart_announcements(true)`, the task listens for restart
  announcements from PCP and NAT-PMP routers. After a restart, it renews
  the mapping within a few seconds.
- If a renewal fails, or the router refuses for a short-term reason, the
  mapping stays until it expires. Other refusals, or a new local address
  or gateway, end the mapping, and the task asks for a new one.
- `last_error()` gives one cause for each protocol tried. It is `None`
  after an attempt works. After `stop()`, it shows if the release failed.
- `status()` gives the mapping and the last error together, in one
  snapshot, so they always agree.
- `stop()` releases the mapping and waits up to two seconds. Dropping the
  handle releases it in the background.
- If the external port is in use, UPnP tries other ports. If the gateway
  only grants permanent UPnP mappings, it asks for one, and `stop()` still
  removes it.

## Limits

- **Use a lifetime of one week or less.** Some UPnP gateways cut longer
  leases to one week and do not say so. The mapping then expires before
  the task renews it.
- IPv4 only.
- PCP and NAT-PMP need the default gateway, which the crate finds only on
  Linux and macOS. Other systems use only UPnP.
- Behind a second NAT, the router's external address is private, and the
  port is not reachable from the internet.
- UPnP uses a gateway only if its URLs are on the host that answered the
  SSDP search.

## Features

| Feature                 | Default | What it adds                                 |
| ----------------------- | ------- | -------------------------------------------- |
| `pcp`                   | yes     | PCP                                          |
| `upnp`                  | yes     | UPnP IGD                                     |
| `nat-pmp`               | yes     | NAT-PMP, which `Config::methods` must list   |
| `restart-announcements` | no      | `Config::restart_announcements`              |

A build needs at least one protocol. To leave protocols out, turn off the
default features and list the ones you want:

```toml
port-control-client = { version = "0.2", default-features = false, features = ["upnp"] }
```

`Config::DEFAULT_METHODS` holds only the protocols that are built. A
protocol that `Config::methods` lists but that is not built fails with
`ErrorKind::NotBuilt`.

## Dependencies

`tokio`, `tracing`, `libc` on macOS (the routing table), `httparse` and
`roxmltree` with `upnp`, `getrandom` with `pcp` or `upnp` (PCP nonces and
random ports), and `socket2` with `restart-announcements`.

## Security

To report a problem, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), as you choose. Unless
you state otherwise, a contribution you submit is licensed the same way,
with no other terms.

[RFC 6886]: https://www.rfc-editor.org/rfc/rfc6886
[RFC 6887]: https://www.rfc-editor.org/rfc/rfc6887
