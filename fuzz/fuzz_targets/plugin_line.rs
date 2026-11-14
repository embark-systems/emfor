//! A line a plugin writes to stdout.
#![no_main]

libfuzzer_sys::fuzz_target!(|line: &str| {
    emfor_fuzz::on_worker_stack(|| emfor_daemon::plugin::host::fuzz_line(line));
});
