use super::*;
use tokio::task::JoinHandle;

use crate::fake::responder;

const REPLY: &[u8] = b"HTTP/1.1 200 OK\r\n\
        CACHE-CONTROL: max-age=120\r\n\
        ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
        USN: uuid:x::urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
        EXT:\r\n\
        SERVER: Linux UPnP/1.1 MiniUPnPd/2.3\r\n\
        Location: http://192.168.1.1:5000/rootDesc.xml\r\n\
        \r\n";

#[test]
fn reply() {
    let router = Ipv4Addr::new(192, 168, 1, 1);
    let url = parse_reply(REPLY, router).unwrap();
    assert_eq!(url.to_string(), "http://192.168.1.1:5000/rootDesc.xml");
}

#[test]
fn location_must_be_the_sender() {
    assert_eq!(parse_reply(REPLY, Ipv4Addr::new(192, 168, 1, 2)), None);
}

#[test]
fn not_a_reply() {
    let router = Ipv4Addr::new(192, 168, 1, 1);
    assert_eq!(parse_reply(request(TARGETS[0]).as_bytes(), router), None);
    assert_eq!(parse_reply(b"HTTP/1.1 404 Not Found\r\n\r\n", router), None);
    // Not a success, even with a location.
    let not_found = b"HTTP/1.1 404 Not Found\r\n\
        LOCATION: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
    assert_eq!(parse_reply(not_found, router), None);
    assert_eq!(parse_reply(b"\x00garbage", router), None);
}

/// Answers each search with a datagram that is not a reply, then a
/// reply from 127.0.0.1 for each path.
async fn answering(paths: Vec<String>) -> (SocketAddrV4, JoinHandle<()>) {
    responder(move |_| {
        let mut replies = vec![b"not a reply".to_vec()];
        replies.extend(paths.iter().map(|path| {
            format!("HTTP/1.1 200 OK\r\nLOCATION: http://127.0.0.1:5000{path}\r\n\r\n").into_bytes()
        }));
        replies
    })
    .await
}

#[tokio::test]
async fn one_host_gets_two_places() {
    let paths = (0..10).map(|i| format!("/d{i}.xml")).collect();
    let (dest, _task) = answering(paths).await;
    let found = search(dest, None, Duration::from_millis(300))
        .await
        .unwrap();
    // Both searches get the same ten replies.
    let got: Vec<_> = found.iter().map(|u| u.path.as_str()).collect();
    assert_eq!(got, ["/d0.xml", "/d1.xml"]);
}

#[test]
fn at_most_eight_gateways_in_order() {
    let url =
        |host: u8, path: &str| Url::parse(&format!("http://10.0.0.{host}:5000{path}")).unwrap();
    let mut found = Vec::new();
    for host in 1..=10 {
        keep(&mut found, url(host, "/a.xml"));
        keep(&mut found, url(host, "/a.xml"));
    }
    let want: Vec<_> = (1..=8).map(|host| url(host, "/a.xml")).collect();
    assert_eq!(found, want);
}

#[tokio::test]
async fn a_reply_larger_than_the_buffer_is_skipped() {
    let (dest, _task) = responder(|_| {
        // A whole reply, but too large: cut short, it does not parse.
        let mut large = b"HTTP/1.1 200 OK\r\nX-PAD: ".to_vec();
        large.extend_from_slice(&[b'x'; MAX_REPLY]);
        large.extend_from_slice(b"\r\nLOCATION: http://127.0.0.1:5000/large.xml\r\n\r\n");
        let reply = b"HTTP/1.1 200 OK\r\nLOCATION: http://127.0.0.1:5000/a.xml\r\n\r\n";
        vec![large, reply.to_vec()]
    })
    .await;
    let found = search(dest, None, Duration::from_millis(300))
        .await
        .unwrap();
    let got: Vec<_> = found.iter().map(|u| u.path.as_str()).collect();
    assert_eq!(got, ["/a.xml"]);
}

#[tokio::test]
async fn the_default_gateway_ends_the_search() {
    let (dest, _task) = answering(vec!["/a.xml".to_owned()]).await;
    let start = Instant::now();
    let found = search(dest, Some(Ipv4Addr::LOCALHOST), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(found.len(), 1);
}

#[tokio::test]
async fn no_answer() {
    let (dest, _task) = responder(|_| Vec::new()).await;
    let found = search(dest, None, Duration::from_millis(100)).await;
    assert!(matches!(found, Err(Failure::NoUpnpGateway)));
}

#[test]
fn errors_about_one_datagram() {
    assert!(skip(&io::Error::from(ErrorKind::ConnectionReset)));
    assert!(skip(&io::Error::from(ErrorKind::ConnectionRefused)));
    assert!(!skip(&io::Error::from(ErrorKind::PermissionDenied)));
    // A datagram larger than the buffer is an error only on Windows.
    let too_large = io::Error::from_raw_os_error(WSAEMSGSIZE);
    assert_eq!(skip(&too_large), cfg!(windows));
}
