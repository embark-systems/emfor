//! A request to the DNS server, over UDP or TCP (see
//! `emfor_daemon::dns::fuzz_request`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    emfor_fuzz::on_worker_stack(|| emfor_daemon::dns::fuzz_request(data));
});
