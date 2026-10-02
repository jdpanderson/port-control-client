//! For the fuzz target: any datagram as an announcement of each kind. Bytes
//! 24 to 32, when present, give an earlier epoch time and the seconds since
//! then, for the epoch checks.

use tokio::time::Duration;

use super::{Epoch, Kind, parse};

pub(crate) fn run(data: &[u8]) {
    let word = |at: usize| {
        data.get(at..at + 4)
            .map_or(0, |w| u32::from_be_bytes([w[0], w[1], w[2], w[3]]))
    };
    let epoch = Epoch::new(word(24));
    let now = epoch.at + Duration::from_secs(word(28).into());
    assert!(
        !epoch.lost(Kind::Pcp, word(24), epoch.at) && !epoch.lost(Kind::NatPmp, word(24), epoch.at),
        "the same epoch time, with no time passed, is valid"
    );
    for kind in [Kind::Pcp, Kind::NatPmp] {
        if let Some(next) = parse(data, kind) {
            let _ = epoch.lost(kind, next, now);
        }
    }
}
