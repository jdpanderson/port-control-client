use super::*;
#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
use crate::responder::responder;

#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
#[tokio::test]
async fn exchange_skips_other_datagrams() {
    let (server, _task) = responder(|_| vec![b"other".to_vec(), b"reply".to_vec()]).await;
    let socket = Socket::connect(server).await.unwrap();
    assert_eq!(socket.local_ip(), Ipv4Addr::LOCALHOST);
    let reply = socket
        .exchange(b"request", &NAT_PMP_WAITS, |b| {
            (b == b"reply").then(|| Ok(b.to_vec()))
        })
        .await
        .unwrap();
    assert_eq!(reply, b"reply");
}

#[cfg(any(feature = "pcp", feature = "nat-pmp"))]
#[tokio::test]
async fn exchange_times_out() {
    let (server, _task) = responder(|_| Vec::new()).await;
    let socket = Socket::connect(server).await.unwrap();
    let start = Instant::now();
    let reply = socket
        .exchange(b"request", &NAT_PMP_WAITS, |_| Some(Ok(())))
        .await;
    assert!(matches!(reply, Err(Failure::Timeout)));
    assert!(start.elapsed() >= Duration::from_millis(1750));
}

#[test]
fn ipv6_is_not_a_local_address() {
    assert!(v4("[::1]:1".parse().unwrap()).is_err());
}
