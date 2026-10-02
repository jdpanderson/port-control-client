//! Why a request to the router failed: [`Failure`] inside the crate, and
//! [`Error`] for users.

use std::{fmt, io};

use crate::Method;

pub(crate) type Result<T, E = Failure> = std::result::Result<T, E>;

/// Why the last attempt to get, renew or release the mapping failed. See
/// [`PortMapping::last_error`](crate::PortMapping::last_error).
///
/// An attempt to get a mapping tries each protocol in order, so the error
/// has a cause for each protocol that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    causes: Vec<Cause>,
}

impl Error {
    /// `causes` must not be empty.
    pub(crate) fn new(causes: Vec<Cause>) -> Self {
        debug_assert!(!causes.is_empty());
        Self { causes }
    }

    /// What failed, for each protocol tried, in the order tried. Never
    /// empty.
    #[must_use]
    pub fn causes(&self) -> &[Cause] {
        &self.causes
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, cause) in self.causes.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{cause}")?;
        }
        Ok(())
    }
}

/// The causes are not a chain, so there is no source: `Display` shows them
/// all.
impl std::error::Error for Error {}

/// What failed with one protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause {
    method: Option<Method>,
    kind: ErrorKind,
    message: String,
}

impl Cause {
    /// The protocol that failed. `None` for a failure before any protocol
    /// could run: no default gateway, or every protocol turned off.
    #[must_use]
    pub fn method(&self) -> Option<Method> {
        self.method
    }

    /// What kind of failure it was.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.method {
            Some(method) => write!(f, "{method}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// `Display` holds the whole message, so there is no source.
impl std::error::Error for Cause {}

/// The kind of a [`Cause`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A local network error, such as no route to the router.
    Io(io::ErrorKind),
    /// No reply came in time. The router may not support the protocol.
    Timeout,
    /// No IPv4 default gateway was found, so PCP and NAT-PMP could not run.
    NoDefaultGateway,
    /// No UPnP gateway answered the search.
    NoUpnpGateway,
    /// The router's reply did not follow the protocol.
    BadReply,
    /// A UPnP gateway answered with this HTTP status and no UPnP error code.
    HttpStatus(u16),
    /// The router refused the request.
    #[non_exhaustive]
    Refused {
        /// The PCP or NAT-PMP result code, or the UPnP error code.
        code: u16,
        /// The failure may be short-term: the same request may work later.
        temporary: bool,
    },
    /// The local address or the default gateway changed after the mapping
    /// was made.
    NetworkChanged,
    /// The configuration turns every protocol off.
    NoProtocol,
}

/// Why a request to the router failed, inside the crate. [`Failure::cause`]
/// turns it into what users see.
#[derive(Debug)]
pub(crate) enum Failure {
    Io(io::Error),
    Timeout,
    NoDefaultGateway(io::Error),
    NoUpnpGateway,
    /// The reply does not follow the protocol, for this reason.
    BadReply(&'static str),
    HttpStatus(u16),
    Refused {
        code: u16,
        /// The name of the code, for messages.
        name: &'static str,
        temporary: bool,
    },
    NetworkChanged,
    NoProtocol,
}

impl Failure {
    /// The router's answer, or a change in our network, shows the mapping is
    /// gone. Asking again before it expires will not help.
    pub(crate) fn mapping_gone(&self) -> bool {
        matches!(
            self,
            Failure::Refused {
                temporary: false,
                ..
            } | Failure::NetworkChanged
        )
    }

    /// The cause users see, for a failure of `method`.
    pub(crate) fn cause(&self, method: Option<Method>) -> Cause {
        let kind = match self {
            Failure::Io(e) => ErrorKind::Io(e.kind()),
            Failure::Timeout => ErrorKind::Timeout,
            Failure::NoDefaultGateway(_) => ErrorKind::NoDefaultGateway,
            Failure::NoUpnpGateway => ErrorKind::NoUpnpGateway,
            Failure::BadReply(_) => ErrorKind::BadReply,
            Failure::HttpStatus(status) => ErrorKind::HttpStatus(*status),
            Failure::Refused {
                code, temporary, ..
            } => ErrorKind::Refused {
                code: *code,
                temporary: *temporary,
            },
            Failure::NetworkChanged => ErrorKind::NetworkChanged,
            Failure::NoProtocol => ErrorKind::NoProtocol,
        };
        Cause {
            method,
            kind,
            message: self.to_string(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Io(e) => write!(f, "{e}"),
            Failure::Timeout => f.write_str("no reply in time"),
            Failure::NoDefaultGateway(e) => write!(f, "no default gateway: {e}"),
            Failure::NoUpnpGateway => f.write_str("no UPnP gateway answered"),
            Failure::BadReply(why) => write!(f, "bad reply: {why}"),
            Failure::HttpStatus(status) => write!(f, "HTTP status {status}"),
            Failure::Refused { code, name, .. } => write!(f, "error {code} ({name})"),
            Failure::NetworkChanged => f.write_str("the local address or gateway changed"),
            Failure::NoProtocol => f.write_str("PCP, NAT-PMP and UPnP are all off"),
        }
    }
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        Failure::Io(e)
    }
}

#[cfg(test)]
mod tests;
