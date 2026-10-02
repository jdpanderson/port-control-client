# Security policy

## Reporting a problem

Report security problems with GitHub's
[private vulnerability reporting](https://github.com/jdpanderson/port-control-client/security/advisories/new),
not in a public issue. Give the version, and the input that makes the
crate fail. For example: a reply from the network that makes it crash,
hang, use too much memory, or send requests to the wrong host.

## Supported versions

Security fixes go into the latest release.

## What the crate trusts

Nothing that comes from the network. Replies from the router, and from any
host that answers the SSDP search, are untrusted. UPnP URLs are used only
if they are on the host that answered. Restart announcements, if turned
on, count only if they come from the gateway's port 5351. Reads are
limited in size. The parsers are fuzzed (see `fuzz/`).
