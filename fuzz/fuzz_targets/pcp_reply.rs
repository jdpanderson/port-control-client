//! PCP MAP responses.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::pcp_reply(data));
