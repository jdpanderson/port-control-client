use super::*;
use std::net::Ipv4Addr;

use tokio::net::TcpListener;

fn complete(raw: &[u8]) -> Response {
    parse(raw, false).unwrap().unwrap()
}

#[test]
fn content_length() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    assert_eq!(
        complete(raw),
        Response {
            status: 200,
            body: b"hello".to_vec()
        }
    );
    assert_eq!(parse(&raw[..raw.len() - 1], false).unwrap(), None);
    assert!(parse(&raw[..raw.len() - 1], true).is_err());
}

#[test]
fn chunked() {
    let raw = b"HTTP/1.1 500 Internal Server Failure\r\ntransfer-encoding: Chunked\r\n\r\n4\r\nhell\r\n1;x=y\r\no\r\n0\r\n\r\n";
    assert_eq!(
        complete(raw),
        Response {
            status: 500,
            body: b"hello".to_vec()
        }
    );
    assert_eq!(parse(&raw[..raw.len() - 6], false).unwrap(), None);
    let bad = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nhello\r\n0\r\n\r\n";
    assert!(parse(bad, false).is_err());
}

#[test]
fn huge_chunk() {
    // The chunk end is near u64::MAX: no overflow, no panic.
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nFFFFFFFFFFFFFFEC\r\nx";
    assert!(parse(raw, false).is_err());
}

#[test]
fn interim_responses() {
    let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    assert_eq!(complete(raw).body, b"ok");
    assert_eq!(parse(&raw[..30], false).unwrap(), None);
}

#[test]
fn until_closed() {
    let raw = b"HTTP/1.0 200 OK\r\nServer: x\r\n\r\nhello";
    assert_eq!(parse(raw, false).unwrap(), None);
    assert_eq!(parse(raw, true).unwrap().unwrap().body, b"hello");
}

#[test]
fn not_http() {
    assert!(parse(b"\x00\x01garbage\r\n\r\n", false).is_err());
    assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n", false).is_err());
}

#[test]
fn chunk_sizes() {
    // The size line is not complete yet, or not a size.
    assert_eq!(dechunk(b"4").unwrap(), None);
    assert!(dechunk(b"zz\r\n").is_err());
}

/// A server that reads one request, sends `response`, and closes the
/// connection.
async fn serving(response: Vec<u8>) -> Url {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let n = stream.read(&mut buf).await.unwrap();
            request.extend_from_slice(&buf[..n]);
        }
        // The client hangs up early on a response that is too large.
        let _ = stream.write_all(&response).await;
    });
    Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()
}

#[tokio::test]
async fn the_size_limit() {
    // The response ends when the server closes the connection.
    let response = |size| {
        let mut b = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        b.resize(size, b'x');
        b
    };
    // Device descriptions are a few kilobytes: the limit leaves room.
    let got = get(&serving(response(64 * 1024)).await).await.unwrap();
    assert_eq!(got.status, 200);
    let got = get(&serving(response(MAX_RESPONSE)).await).await.unwrap();
    assert_eq!(got.status, 200);
    let got = get(&serving(response(MAX_RESPONSE + 1)).await).await;
    assert!(
        matches!(got, Err(Failure::BadReply("HTTP response too large"))),
        "{got:?}"
    );
}

#[tokio::test]
async fn too_large() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Reads the request, then sends a body with no end, until the client
    // hangs up.
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let n = stream.read(&mut buf).await.unwrap();
            request.extend_from_slice(&buf[..n]);
        }
        let mut out = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        out.resize(64 * 1024, b'x');
        while stream.write_all(&out).await.is_ok() {
            out.fill(b'x');
        }
    });
    let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
    let got = get(&url).await;
    assert!(
        matches!(got, Err(Failure::BadReply("HTTP response too large"))),
        "{got:?}"
    );
}
