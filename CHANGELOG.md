# Changelog

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0]

Initial Release

### Added

- IPv4 port mapping with PCP, NAT-PMP and UPnP IGD. The mapping is renewed
  in the background and released on stop.
- Default gateway lookup on Linux and macOS.
- Renewal when a PCP or NAT-PMP router announces that it restarted, with
  the optional `restart-announcements` feature.