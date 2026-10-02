//! Maps a port on the router until Ctrl-C, then releases it.
//!
//! ```text
//! cargo run --example map -- udp 51820
//! ```

use std::{env, num::NonZeroU16, process::ExitCode, time::Duration};

use port_control_client::{Config, PortMapping, Protocol};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [protocol, port] = args.as_slice() else {
        return usage();
    };
    let protocol = match protocol.to_ascii_lowercase().as_str() {
        "udp" => Protocol::Udp,
        "tcp" => Protocol::Tcp,
        _ => return usage(),
    };
    let Some(port) = port.parse().ok().and_then(NonZeroU16::new) else {
        return usage();
    };

    let config = Config::new(protocol, port).description("port-control-client example");
    let mapping = PortMapping::start(config);
    let mut watch = mapping.watch();
    println!("asking the router to map {protocol} port {port}; Ctrl-C stops");
    // Errors have no notification, so look at the last one each second.
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut shown = None;
    loop {
        tokio::select! {
            changed = watch.changed() => {
                if changed.is_err() {
                    break;
                }
                match *watch.borrow_and_update() {
                    Some(m) => println!("mapped to {} through {}", m.external, m.method),
                    None => println!("no mapping"),
                }
            }
            _ = tick.tick() => {
                let error = mapping.last_error();
                if error != shown {
                    if let Some(error) = &error {
                        println!("last error: {error}");
                    }
                    shown = error;
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    let had_mapping = mapping.mapping().is_some();
    mapping.stop().await;
    if had_mapping {
        match mapping.last_error() {
            Some(error) => println!("can't release: {error}"),
            None => println!("released"),
        }
    }
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("usage: map <udp|tcp> <port>");
    ExitCode::FAILURE
}
