//! The Linux routing table, `/proc/net/route`.

#![no_main]

// The parser only exists on Linux.
#[cfg(target_os = "linux")]
libfuzzer_sys::fuzz_target!(|data: &[u8]| port_control_client::fuzz::proc_route(data));

#[cfg(not(target_os = "linux"))]
libfuzzer_sys::fuzz_target!(|_data: &[u8]| {});
