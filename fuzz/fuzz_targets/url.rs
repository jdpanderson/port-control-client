//! URLs from SSDP replies and device descriptions.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::url(data));
