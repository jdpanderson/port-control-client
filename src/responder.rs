//! A UDP responder, for the unit tests.

use std::net::{Ipv4Addr, SocketAddrV4};

use tokio::{net::UdpSocket, task::JoinHandle};

/// A UDP server on 127.0.0.1 that answers each datagram with the datagrams
/// that `answer` returns for it.
pub(crate) async fn responder(
    answer: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + 'static,
) -> (SocketAddrV4, JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, socket.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        let mut buf = [0; 2048];
        loop {
            let Ok((n, from)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            for reply in answer(&buf[..n]) {
                let _ = socket.send_to(&reply, from).await;
            }
        }
    });
    (addr, task)
}
