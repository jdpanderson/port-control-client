//! For the fuzz target: any bytes as a response. A complete response stays
//! the same when the connection closes, and its body is never larger than
//! the input.

use super::{dechunk, parse};

pub(crate) fn run(data: &[u8]) {
    let closed = parse(data, true);
    if let Ok(Some(response)) = parse(data, false) {
        assert!(response.body.len() <= data.len());
        assert_eq!(closed.ok().flatten(), Some(response));
    }
    let _ = dechunk(data);
}
