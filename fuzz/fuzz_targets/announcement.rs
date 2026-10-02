//! PCP and NAT-PMP restart announcements, and their epoch checks.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::announcement(data));
