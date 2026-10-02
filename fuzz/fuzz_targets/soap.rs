//! SOAP responses and faults, and SOAP request arguments.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::soap(data));
