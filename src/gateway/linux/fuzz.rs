//! For the fuzz target: any text as the routing table.

use super::parse;

pub(crate) fn run(data: &[u8]) {
    let _ = parse(&String::from_utf8_lossy(data));
}
