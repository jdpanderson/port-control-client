//! `http://` URLs whose host is an IPv4 address, as UPnP gateways give them.

use std::{
    fmt,
    net::{Ipv4Addr, SocketAddrV4},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Url {
    pub(crate) addr: SocketAddrV4,
    /// Starts with `/`; may hold a query.
    pub(crate) path: String,
}

impl Url {
    /// Parses `http://a.b.c.d[:port][/path]`. Host names, other schemes and
    /// user names are not accepted.
    pub(crate) fn parse(s: &str) -> Option<Url> {
        let s = s.trim();
        let scheme = s.get(..7)?;
        if !scheme.eq_ignore_ascii_case("http://") {
            return None;
        }
        let rest = &s[7..];
        let (authority, path) = match rest.find('/') {
            Some(i) => rest.split_at(i),
            None => (rest, "/"),
        };
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, 80),
        };
        let ip: Ipv4Addr = host.parse().ok()?;
        if port == 0 {
            return None;
        }
        Url::with_path(SocketAddrV4::new(ip, port), path)
    }

    /// Resolves `reference`, a URL or a path, against this URL.
    pub(crate) fn join(&self, reference: &str) -> Option<Url> {
        let r = reference.trim();
        if r.is_empty() || r.starts_with("//") {
            return None;
        }
        if r.starts_with('/') {
            return Url::with_path(self.addr, r);
        }
        if r.contains("://") {
            return Url::parse(r);
        }
        // A relative path replaces the last segment of this URL's path.
        let base = self.path.split(['?', '#']).next().unwrap_or("/");
        let dir = &base[..base.rfind('/').map_or(0, |i| i + 1)];
        Url::with_path(self.addr, &format!("{dir}{r}"))
    }

    /// The path goes into the request line, so it must not hold spaces or
    /// control characters.
    fn with_path(addr: SocketAddrV4, path: &str) -> Option<Url> {
        let path = path.split('#').next().unwrap_or_default();
        let printable = path.bytes().all(|b| b.is_ascii_graphic());
        (path.starts_with('/') && printable).then(|| Url {
            addr,
            path: path.to_owned(),
        })
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://{}{}", self.addr, self.path)
    }
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
