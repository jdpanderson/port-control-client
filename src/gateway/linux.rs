//! Linux: the main routing table, from `/proc/net/route`.

use std::{io, net::Ipv4Addr};

const RTF_UP: u32 = 0x1;
const RTF_GATEWAY: u32 = 0x2;

#[cfg(target_os = "linux")]
pub(crate) fn default_gateway() -> io::Result<Ipv4Addr> {
    parse(&std::fs::read_to_string("/proc/net/route")?)
}

/// The gateway of the default route with the lowest metric.
///
/// After a header line, each line has: interface, destination, gateway,
/// flags, reference count, use, metric, mask, and more. Addresses and flags
/// are hexadecimal; addresses are in host byte order.
fn parse(table: &str) -> io::Result<Ipv4Addr> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [_, dest, gateway, flags, _, _, metric, mask, ..] = fields[..] else {
                return None;
            };
            let up = RTF_UP | RTF_GATEWAY;
            let default = hex(dest)? == 0 && hex(mask)? == 0 && hex(flags)? & up == up;
            let ip = Ipv4Addr::from(hex(gateway)?.to_ne_bytes());
            default.then_some((metric.parse::<u32>().ok()?, ip))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, ip)| ip)
        .ok_or_else(super::no_default_route)
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

// The test table is in little-endian byte order.
#[cfg(all(test, target_endian = "little"))]
mod tests;
