//! Fake routers on 127.0.0.1, for the tests.

use std::{
    fmt::Write as _,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU32, Ordering},
    },
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    task::JoinHandle,
    time::Instant,
};

/// The external address that the fake routers report.
pub(crate) const EXTERNAL_IP: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 5);
/// The external port that the fake PCP server grants when the client
/// suggests none.
pub(crate) const GRANTED_PORT: u16 = 40000;

/// What the fake PCP and NAT-PMP server answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Speaks {
    /// Nothing.
    Silent = 0,
    /// PCP only. NAT-PMP requests get no answer.
    Pcp = 1,
    /// NAT-PMP only. PCP requests get "unsupported version".
    NatPmp = 2,
    /// Both, but every request gets "not authorized" (result 2).
    Refuses = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PmpRequest {
    PcpMap {
        nonce: [u8; 12],
        lifetime: u32,
        suggested_port: u16,
    },
    NatPmpMap {
        lifetime: u32,
        suggested_port: u16,
    },
    NatPmpExternal,
}

/// A PCP and NAT-PMP server, on one UDP port as on a router.
pub(crate) struct FakePmp {
    pub(crate) port: u16,
    #[cfg(feature = "restart-announcements")]
    socket: Arc<UdpSocket>,
    speaks: Arc<AtomicU8>,
    external: Arc<AtomicU32>,
    /// The epoch time at an instant: it counts up from there.
    #[cfg(feature = "restart-announcements")]
    epoch: Arc<Mutex<(u32, Instant)>>,
    seen: Arc<Mutex<Vec<PmpRequest>>>,
    task: JoinHandle<()>,
}

/// The fake server's epoch time when it starts: it has been up a while.
const UPTIME: u32 = 1000;

fn epoch_now(epoch: &Mutex<(u32, Instant)>) -> u32 {
    let (start, at) = *epoch.lock().unwrap();
    start + u32::try_from(at.elapsed().as_secs()).unwrap()
}

impl FakePmp {
    pub(crate) async fn start(speaks: Speaks) -> Self {
        let socket = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
        let port = socket.local_addr().unwrap().port();
        let speaks = Arc::new(AtomicU8::new(speaks as u8));
        let external = Arc::new(AtomicU32::new(EXTERNAL_IP.into()));
        let epoch = Arc::new(Mutex::new((UPTIME, Instant::now())));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn({
            let (socket, speaks, external) = (socket.clone(), speaks.clone(), external.clone());
            let (epoch, seen) = (epoch.clone(), seen.clone());
            async move {
                let mut buf = [0; 1100];
                loop {
                    // Windows reports ICMP errors for earlier replies here.
                    let Ok((n, from)) = socket.recv_from(&mut buf).await else {
                        continue;
                    };
                    let speaks = speaks.load(Ordering::SeqCst);
                    let external = Ipv4Addr::from(external.load(Ordering::SeqCst));
                    let epoch = epoch_now(&epoch);
                    let (request, reply) = pmp_reply(&buf[..n], speaks, external, epoch);
                    seen.lock().unwrap().push(request);
                    if let Some(reply) = reply {
                        let _ = socket.send_to(&reply, from).await;
                    }
                }
            }
        });
        Self {
            port,
            #[cfg(feature = "restart-announcements")]
            socket,
            speaks,
            external,
            #[cfg(feature = "restart-announcements")]
            epoch,
            seen,
            task,
        }
    }

    /// Loses every mapping, as in a reboot: the epoch time starts again
    /// from zero.
    #[cfg(feature = "restart-announcements")]
    pub(crate) fn restart(&self) {
        *self.epoch.lock().unwrap() = (0, Instant::now());
    }

    /// Sends a restart announcement to `to`, in the protocol the server
    /// speaks, from the server's port.
    #[cfg(feature = "restart-announcements")]
    pub(crate) async fn announce(&self, to: SocketAddrV4) {
        let epoch = epoch_now(&self.epoch).to_be_bytes();
        let announcement = if self.speaks.load(Ordering::SeqCst) == Speaks::Pcp as u8 {
            let mut b = vec![2, 0x80, 0, 0, 0, 0, 0, 0];
            b.extend_from_slice(&epoch);
            b.resize(24, 0);
            b
        } else {
            let mut b = vec![0, 128, 0, 0];
            b.extend_from_slice(&epoch);
            b.extend_from_slice(&EXTERNAL_IP.octets());
            b
        };
        self.socket.send_to(&announcement, to).await.unwrap();
    }

    pub(crate) fn speak(&self, speaks: Speaks) {
        self.speaks.store(speaks as u8, Ordering::SeqCst);
    }

    /// The external address that later replies report.
    pub(crate) fn set_external(&self, ip: Ipv4Addr) {
        self.external.store(ip.into(), Ordering::SeqCst);
    }

    pub(crate) fn seen(&self) -> Vec<PmpRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// The requests, without the sends that repeat a request. A slow test
    /// machine can make the client send again.
    pub(crate) fn requests(&self) -> Vec<PmpRequest> {
        let mut seen = self.seen();
        seen.dedup();
        seen
    }
}

impl Drop for FakePmp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn pmp_reply(
    b: &[u8],
    speaks: u8,
    external: Ipv4Addr,
    epoch: u32,
) -> (PmpRequest, Option<Vec<u8>>) {
    let refuses = speaks == Speaks::Refuses as u8;
    let nat_pmp = speaks == Speaks::NatPmp as u8 || refuses;
    let result = u8::from(refuses) * 2;
    let be16 = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
    let be32 = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    match b[0] {
        2 => {
            let lifetime = be32(4);
            let suggested_port = be16(42);
            let request = PmpRequest::PcpMap {
                nonce: b[24..36].try_into().unwrap(),
                lifetime,
                suggested_port,
            };
            let reply = if speaks == Speaks::Pcp as u8 || refuses {
                let mut r = b.to_vec();
                r[1] |= 0x80;
                r[2..24].fill(0);
                r[3] = result;
                r[4..8].copy_from_slice(&lifetime.to_be_bytes());
                r[8..12].copy_from_slice(&epoch.to_be_bytes());
                let port = if suggested_port == 0 {
                    GRANTED_PORT
                } else {
                    suggested_port
                };
                // A delete's reply copies the request's zero address and
                // port (RFC 6887, section 15).
                if lifetime != 0 {
                    r[42..44].copy_from_slice(&port.to_be_bytes());
                    r[44..60].copy_from_slice(&external.to_ipv6_mapped().octets());
                }
                Some(r)
            } else if speaks == Speaks::NatPmp as u8 {
                Some(vec![0, 0x80 | b[1], 0, 1, 0, 0, 0, 0])
            } else {
                None
            };
            (request, reply)
        }
        0 if b[1] == 0 => {
            let mut r = vec![0, 128, 0, result];
            r.extend_from_slice(&epoch.to_be_bytes());
            r.extend_from_slice(&external.octets());
            (PmpRequest::NatPmpExternal, nat_pmp.then_some(r))
        }
        0 => {
            let (lifetime, suggested_port) = (be32(8), be16(6));
            let port = if lifetime == 0 { 0 } else { suggested_port };
            let mut r = vec![0, 128 + b[1], 0, result];
            r.extend_from_slice(&epoch.to_be_bytes());
            r.extend_from_slice(&b[4..6]);
            r.extend_from_slice(&port.to_be_bytes());
            r.extend_from_slice(&lifetime.to_be_bytes());
            let request = PmpRequest::NatPmpMap {
                lifetime,
                suggested_port,
            };
            (request, nat_pmp.then_some(r))
        }
        v => panic!("unknown version {v}"),
    }
}

/// How the fake UPnP gateway behaves.
#[derive(Clone, Debug, Default)]
pub(crate) struct IgdOptions {
    /// Refuse leases that are not permanent (error 725).
    pub(crate) permanent_only: bool,
    /// External ports that another host has (error 718).
    pub(crate) taken: Vec<u16>,
    /// Send chunked responses.
    pub(crate) chunked: bool,
    /// Point the search reply at a description that does not exist.
    pub(crate) missing_description: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IgdCall {
    GetExternal,
    Add {
        external_port: u16,
        internal_port: u16,
        client: String,
        lease: u32,
    },
    Delete {
        external_port: u16,
    },
}

/// A UPnP gateway: an SSDP responder and an HTTP server.
pub(crate) struct FakeIgd {
    pub(crate) ssdp: SocketAddrV4,
    calls: Arc<Mutex<Vec<IgdCall>>>,
    answers: Arc<Mutex<Answers>>,
    tasks: [JoinHandle<()>; 2],
}

impl FakeIgd {
    pub(crate) async fn start(options: IgdOptions) -> Self {
        let http = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let http_port = http.local_addr().unwrap().port();
        let ssdp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let ssdp_port = ssdp.local_addr().unwrap().port();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let answers = Arc::new(Mutex::new(Answers {
            add_error: 0,
            delete_error: 0,
            external: EXTERNAL_IP,
        }));
        let path = if options.missing_description {
            "/missing.xml"
        } else {
            "/rootDesc.xml"
        };
        let ssdp_task = tokio::spawn(async move {
            let mut buf = [0; 2048];
            loop {
                let Ok((n, from)) = ssdp.recv_from(&mut buf).await else {
                    continue;
                };
                if buf[..n].starts_with(b"M-SEARCH") {
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\n\
                         ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
                         LOCATION: http://127.0.0.1:{http_port}{path}\r\n\r\n"
                    );
                    let _ = ssdp.send_to(reply.as_bytes(), from).await;
                }
            }
        });
        let http_task = tokio::spawn({
            let (calls, answers) = (calls.clone(), answers.clone());
            let options = Arc::new(options);
            async move {
                loop {
                    let (stream, _) = http.accept().await.unwrap();
                    let answers = *answers.lock().unwrap();
                    tokio::spawn(serve(stream, calls.clone(), options.clone(), answers));
                }
            }
        });
        Self {
            ssdp: SocketAddrV4::new(Ipv4Addr::LOCALHOST, ssdp_port),
            calls,
            answers,
            tasks: [ssdp_task, http_task],
        }
    }

    pub(crate) fn calls(&self) -> Vec<IgdCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Refuse every later AddPortMapping with this error code.
    pub(crate) fn refuse(&self, code: u16) {
        self.answers.lock().unwrap().add_error = code;
    }

    /// Refuse every later DeletePortMapping with this error code.
    pub(crate) fn refuse_deletes(&self, code: u16) {
        self.answers.lock().unwrap().delete_error = code;
    }

    /// The external address that later answers report. 0.0.0.0 is what a
    /// gateway reports when its WAN link is down.
    pub(crate) fn set_external(&self, ip: Ipv4Addr) {
        self.answers.lock().unwrap().external = ip;
    }
}

/// How the fake gateway answers. Tests change it while it runs.
#[derive(Clone, Copy, Debug)]
struct Answers {
    /// The error code for AddPortMapping, or 0.
    add_error: u16,
    /// The error code for DeletePortMapping, or 0.
    delete_error: u16,
    external: Ipv4Addr,
}

impl Drop for FakeIgd {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
    <deviceList><device>
      <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
      <deviceList><device>
        <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
        <serviceList><service>
          <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
          <controlURL>/ctl/IPConn</controlURL>
        </service></serviceList>
      </device></deviceList>
    </device></deviceList>
  </device>
</root>"#;

async fn serve(
    mut stream: TcpStream,
    calls: Arc<Mutex<Vec<IgdCall>>>,
    options: Arc<IgdOptions>,
    answers: Answers,
) {
    let mut buf = Vec::new();
    let mut chunk = [0; 4096];
    let (method, path, action, body) = loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert_ne!(
            n, 0,
            "the client closed the connection before the request ended"
        );
        buf.extend_from_slice(&chunk[..n]);
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        let Ok(httparse::Status::Complete(start)) = request.parse(&buf) else {
            continue;
        };
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| String::from_utf8_lossy(h.value).trim().to_owned())
        };
        let length: usize = header("content-length").map_or(0, |v| v.parse().unwrap());
        if buf.len() < start + length {
            continue;
        }
        let action = header("soapaction");
        let method = request.method.unwrap().to_owned();
        let path = request.path.unwrap().to_owned();
        break (method, path, action, buf[start..start + length].to_vec());
    };
    let (status, body) = match (method.as_str(), path.as_str()) {
        ("GET", "/rootDesc.xml") => (200, DESCRIPTION.to_owned()),
        ("POST", "/ctl/IPConn") => soap(&calls, &options, answers, &action.unwrap(), &body),
        _ => (404, String::new()),
    };
    let mut out = format!("HTTP/1.1 {status} X\r\nContent-Type: text/xml\r\nConnection: close\r\n");
    if options.chunked {
        out.push_str("Transfer-Encoding: chunked\r\n\r\n");
        let (a, b) = body.split_at(body.len() / 2);
        for part in [a, b].into_iter().filter(|p| !p.is_empty()) {
            let _ = write!(out, "{:x}\r\n{part}\r\n", part.len());
        }
        out.push_str("0\r\n\r\n");
    } else {
        let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    }
    stream.write_all(out.as_bytes()).await.unwrap();
}

fn soap(
    calls: &Mutex<Vec<IgdCall>>,
    options: &IgdOptions,
    answers: Answers,
    action: &str,
    body: &[u8],
) -> (u16, String) {
    let (_, name) = action.trim_matches('"').split_once('#').unwrap();
    let xml = std::str::from_utf8(body).unwrap();
    let doc = roxmltree::Document::parse(xml).unwrap();
    let call = doc
        .descendants()
        .find(|n| n.tag_name().name() == name)
        .unwrap();
    let arg = |a: &str| {
        call.children()
            .find(|c| c.tag_name().name() == a)
            .and_then(|c| c.text())
            .unwrap_or_default()
            .to_owned()
    };
    let mut calls = calls.lock().unwrap();
    match name {
        "GetExternalIPAddress" => {
            calls.push(IgdCall::GetExternal);
            let ip = answers.external;
            (
                200,
                response(
                    name,
                    &format!("<NewExternalIPAddress>{ip}</NewExternalIPAddress>"),
                ),
            )
        }
        "AddPortMapping" => {
            let external_port: u16 = arg("NewExternalPort").parse().unwrap();
            let lease: u32 = arg("NewLeaseDuration").parse().unwrap();
            calls.push(IgdCall::Add {
                external_port,
                internal_port: arg("NewInternalPort").parse().unwrap(),
                client: arg("NewInternalClient"),
                lease,
            });
            if answers.add_error != 0 {
                fault(answers.add_error)
            } else if options.permanent_only && lease != 0 {
                fault(725)
            } else if options.taken.contains(&external_port) {
                fault(718)
            } else {
                (200, response(name, ""))
            }
        }
        "DeletePortMapping" => {
            let external_port = arg("NewExternalPort").parse().unwrap();
            calls.push(IgdCall::Delete { external_port });
            if answers.delete_error != 0 {
                fault(answers.delete_error)
            } else {
                (200, response(name, ""))
            }
        }
        _ => fault(401),
    }
}

fn envelope(body: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body>{body}</s:Body></s:Envelope>"#
    )
}

fn response(action: &str, args: &str) -> String {
    envelope(&format!(
        r#"<u:{action}Response xmlns:u="urn:schemas-upnp-org:service:WANIPConnection:1">{args}</u:{action}Response>"#
    ))
}

fn fault(code: u16) -> (u16, String) {
    let detail = format!(
        r#"<UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>x</errorDescription></UPnPError>"#
    );
    let body = envelope(&format!(
        "<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail>{detail}</detail></s:Fault>"
    ));
    (500, body)
}

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
