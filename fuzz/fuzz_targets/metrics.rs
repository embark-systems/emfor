//! The bytes a client sends to the metrics listener.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    emfor_daemon::metrics::fuzz_request(data);
});
