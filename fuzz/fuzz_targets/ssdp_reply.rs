//! SSDP search replies.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::ssdp_reply(data));
