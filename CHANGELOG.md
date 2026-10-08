# Changelog

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `PortMapping::status` and `Status`: the mapping and the last error in
  one snapshot, so they always agree. Reading `mapping()` and then
  `last_error()` could show a new error with an old mapping.
- `PortMapping::local_port`, the local port that the mapping is for.

### Changed

- `mapping()` and `last_error()` read the same snapshot as `status()`.
  What they return does not change.

- `tracing` is built without its default features, so `tracing-attributes`
  and its `syn` are no longer dependencies.
- CI fuzzes each target for 10 seconds, not 30, so the fuzz job takes
  about 2 minutes instead of over 4.

## [0.2.1] - 2026-10-05

### Changed

- No changes to the library. The tests check more cases with less code,
  and the lockfile has newer patch versions of `libc`, `mio` and `tokio`.

## [0.2.0] - 2026-10-05

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