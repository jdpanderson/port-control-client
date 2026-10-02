//! NAT-PMP mapping and external address responses.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::nat_pmp_reply(data));
