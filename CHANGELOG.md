# Changelog

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Config::methods` sets which protocols to try, and in what order.
- `Config::DEFAULT_METHODS`, the default order.
- Features `pcp`, `nat-pmp` and `upnp`, all on by default. A build needs at
  least one. Without `upnp`, `httparse` and `roxmltree` are not built.
- `ErrorKind::NotBuilt`, for a protocol in `Config::methods` whose feature
  is off.

### Changed

- **Breaking:** `Config::methods` replaces the `Config::pcp`,
  `Config::nat_pmp` and `Config::upnp` switches, which are removed. For
  example, `.upnp(false)` becomes `.methods([Method::Pcp, Method::NatPmp])`.
- **Breaking:** with `default-features = false`, no protocol is built:
  add `pcp`, `nat-pmp` or `upnp`.
- **Breaking:** NAT-PMP is off by default. The default is PCP, then UPnP.
  For the old behavior, use
  `.methods([Method::Pcp, Method::NatPmp, Method::Upnp])`.

## [0.1.0]

Initial Release

### Added

- IPv4 port mapping with PCP, NAT-PMP and UPnP IGD. The mapping is renewed
  in the background and released on stop.
- Default gateway lookup on Linux and macOS.
- Renewal when a PCP or NAT-PMP router announces that it restarted, with
  the optional `restart-announcements` feature.