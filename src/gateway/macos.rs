//! macOS: the routing table, from `sysctl`.
//!
//! The dump is a list of route messages. Each is an `rt_msghdr` and then
//! the sockaddrs that its `rtm_addrs` bits name, each padded to 4 bytes.

use std::{
    io,
    mem::{offset_of, size_of},
    net::Ipv4Addr,
    ptr,
};

use libc::rt_msghdr;

const HEADER_LEN: usize = size_of::<rt_msghdr>();

pub(crate) fn default_gateway() -> io::Result<Ipv4Addr> {
    parse(&dump()?)
}

/// The IPv4 routes that have a gateway.
#[expect(unsafe_code, reason = "the standard library has no sysctl")]
fn dump() -> io::Result<Vec<u8>> {
    let mut mib = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_INET,
        libc::NET_RT_FLAGS,
        libc::RTF_GATEWAY,
    ];
    let mib_len = mib.len() as libc::c_uint;
    // The table can grow between the call for the size and the call for
    // the data. Then the second call fails with ENOMEM, and we ask again.
    for _ in 0..3 {
        let mut len: libc::size_t = 0;
        // SAFETY: `mib` holds `mib_len` integers. With a null buffer, sysctl
        // only writes the size it needs to `len`.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib_len,
                ptr::null_mut(),
                &raw mut len,
                ptr::null_mut(),
                0,
            )
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u8; len];
        // SAFETY: `buf` has `len` bytes, and sysctl writes at most `len`
        // bytes to it, then sets `len` to the number it wrote.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib_len,
                buf.as_mut_ptr().cast(),
                &raw mut len,
                ptr::null_mut(),
                0,
            )
        };
        if r == 0 {
            buf.truncate(len);
            return Ok(buf);
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ENOMEM) {
            return Err(e);
        }
    }
    Err(io::Error::other("the routing table kept growing"))
}

/// The gateway of the default route. A scoped route (`RTF_IFSCOPE`) is
/// bound to one interface; the unscoped one is the system's choice.
fn parse(mut buf: &[u8]) -> io::Result<Ipv4Addr> {
    let mut scoped = None;
    while !buf.is_empty() {
        let len = buf
            .get(..2)
            .map(|b| usize::from(u16::from_ne_bytes([b[0], b[1]])))
            .filter(|len| (HEADER_LEN..=buf.len()).contains(len))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "bad route message length")
            })?;
        let (msg, rest) = buf.split_at(len);
        buf = rest;
        if i32::from(msg[offset_of!(rt_msghdr, rtm_version)]) != libc::RTM_VERSION {
            continue;
        }
        let flags = i32_at(msg, offset_of!(rt_msghdr, rtm_flags));
        let up = libc::RTF_UP | libc::RTF_GATEWAY;
        if flags & up != up {
            continue;
        }
        let addrs = sockaddrs(
            &msg[HEADER_LEN..],
            i32_at(msg, offset_of!(rt_msghdr, rtm_addrs)),
        );
        let [dst, gateway, mask] =
            [libc::RTAX_DST, libc::RTAX_GATEWAY, libc::RTAX_NETMASK].map(|i| addrs[i as usize]);
        // The default route: destination 0.0.0.0 and a zero netmask.
        let default =
            dst.and_then(inet) == Some(Ipv4Addr::UNSPECIFIED) && mask.is_some_and(zero_mask);
        let Some(gateway) = gateway.and_then(inet).filter(|_| default) else {
            continue;
        };
        if flags & libc::RTF_IFSCOPE == 0 {
            return Ok(gateway);
        }
        scoped.get_or_insert(gateway);
    }
    scoped.ok_or_else(super::no_default_route)
}

fn i32_at(msg: &[u8], offset: usize) -> i32 {
    let mut b = [0; 4];
    b.copy_from_slice(&msg[offset..offset + 4]);
    i32::from_ne_bytes(b)
}

/// The sockaddrs after a message header, by `RTAX_*` index. A sockaddr
/// starts with its length; length 0 still takes 4 bytes.
fn sockaddrs(mut b: &[u8], addrs: i32) -> [Option<&[u8]>; libc::RTAX_MAX as usize] {
    let mut out = [None; libc::RTAX_MAX as usize];
    for (i, slot) in out.iter_mut().enumerate() {
        if addrs & (1 << i) == 0 {
            continue;
        }
        let Some(&len) = b.first() else {
            break;
        };
        let len = usize::from(len);
        let Some(sa) = b.get(..len) else {
            break;
        };
        *slot = Some(sa);
        let padded = if len == 0 { 4 } else { len.next_multiple_of(4) };
        b = b.get(padded..).unwrap_or_default();
    }
    out
}

/// The address in an `AF_INET` sockaddr: length, family, port, address.
fn inet(sa: &[u8]) -> Option<Ipv4Addr> {
    (sa.len() >= 8 && i32::from(sa[1]) == libc::AF_INET)
        .then(|| Ipv4Addr::new(sa[4], sa[5], sa[6], sa[7]))
}

/// The kernel cuts a netmask sockaddr after its last nonzero byte, so a
/// zero mask may be shorter than a sockaddr, or empty.
fn zero_mask(sa: &[u8]) -> bool {
    sa.iter().skip(4).all(|&b| b == 0)
}

#[cfg(test)]
mod tests;
