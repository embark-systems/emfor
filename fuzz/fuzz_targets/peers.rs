//! A peer datagram as received from the network.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    emfor_fuzz::on_worker_stack(|| emfor_daemon::peers::fuzz_decode(data));
});
