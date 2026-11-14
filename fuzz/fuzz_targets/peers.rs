//! A peer datagram as received from the network.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    emfor_daemon::peers::fuzz_decode(data);
});
