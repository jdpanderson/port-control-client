//! HTTP responses from a UPnP gateway.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::http_response(data));
