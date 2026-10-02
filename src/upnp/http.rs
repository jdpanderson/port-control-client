//! A small HTTP/1.1 client for the gateway's description and control URLs:
//! plain HTTP on the local network, one request per connection.

use std::fmt::Write as _;

use httparse::Status;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Duration, timeout},
};

use super::url::Url;
use crate::error::{Failure, Result};

/// Larger responses are refused. Device descriptions are a few kilobytes.
const MAX_RESPONSE: usize = 256 * 1024;
/// For the whole request, from connecting to the end of the response.
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

pub(crate) async fn get(url: &Url) -> Result<Response> {
    request(url, "GET", &[], None).await
}

pub(crate) async fn post(url: &Url, headers: &[(&str, &str)], body: &[u8]) -> Result<Response> {
    request(url, "POST", headers, Some(body)).await
}

async fn request(
    url: &Url,
    method: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<Response> {
    let mut head = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        url.path, url.addr
    );
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if let Some(body) = body {
        let _ = write!(head, "Content-Length: {}\r\n", body.len());
    }
    head.push_str("\r\n");

    let exchange = async {
        let mut stream = TcpStream::connect(url.addr).await?;
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body.unwrap_or_default()).await?;
        let mut buf = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            let n = stream.read(&mut chunk).await?;
            if buf.len() + n > MAX_RESPONSE {
                return Err(Failure::BadReply("HTTP response too large"));
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(response) = parse(&buf, n == 0)? {
                return Ok(response);
            }
        }
    };
    timeout(TIMEOUT, exchange)
        .await
        .map_err(|_| Failure::Timeout)?
}

/// The response in `buf`, once it is complete. `closed`: the server has
/// closed the connection, so no more bytes will come.
fn parse(mut buf: &[u8], closed: bool) -> Result<Option<Response>> {
    let incomplete = |what| {
        if closed {
            Err(Failure::BadReply(what))
        } else {
            Ok(None)
        }
    };
    loop {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut response = httparse::Response::new(&mut headers);
        let start = match response.parse(buf) {
            Ok(Status::Complete(n)) => n,
            Ok(Status::Partial) => return incomplete("incomplete HTTP headers"),
            Err(_) => return Err(Failure::BadReply("bad HTTP response")),
        };
        let status = response.code.unwrap_or_default();
        // An interim response, such as 100 Continue, has no body; the
        // final response follows it.
        if (100..200).contains(&status) {
            buf = &buf[start..];
            continue;
        }
        let header = |name: &str| {
            response
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .and_then(|h| std::str::from_utf8(h.value).ok())
                .map(str::trim)
        };
        let body = &buf[start..];
        let chunked =
            header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
        let body = if chunked {
            match dechunk(body)? {
                Some(body) => body,
                None => return incomplete("incomplete chunked body"),
            }
        } else if let Some(length) = header("content-length") {
            let length: usize = length
                .parse()
                .map_err(|_| Failure::BadReply("bad Content-Length"))?;
            match body.get(..length) {
                Some(body) => body.to_vec(),
                None => return incomplete("incomplete HTTP body"),
            }
        } else if closed {
            body.to_vec()
        } else {
            return Ok(None);
        };
        return Ok(Some(Response { status, body }));
    }
}

/// The body of a chunked response, once the last chunk has come.
fn dechunk(mut b: &[u8]) -> Result<Option<Vec<u8>>> {
    const BAD: Failure = Failure::BadReply("bad chunked body");
    let mut out = Vec::new();
    loop {
        let (used, size) = match httparse::parse_chunk_size(b) {
            Ok(Status::Complete(c)) => c,
            Ok(Status::Partial) => return Ok(None),
            Err(_) => return Err(BAD),
        };
        if size == 0 {
            // Trailers may follow; we have no use for them.
            return Ok(Some(out));
        }
        // A chunk larger than a whole response can never be read. Refusing
        // it also keeps the sums below far from overflow.
        let size = match usize::try_from(size) {
            Ok(size) if size <= MAX_RESPONSE => size,
            _ => return Err(BAD),
        };
        let end = used + size;
        let Some(after) = b.get(end..end + 2) else {
            return Ok(None);
        };
        if after != b"\r\n" {
            return Err(BAD);
        }
        out.extend_from_slice(&b[used..end]);
        b = &b[end + 2..];
    }
}

#[cfg(fuzzing)]
pub(crate) mod fuzz;

#[cfg(test)]
mod tests;
