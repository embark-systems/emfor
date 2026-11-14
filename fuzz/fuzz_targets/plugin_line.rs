//! A line a plugin writes to stdout.
#![no_main]

libfuzzer_sys::fuzz_target!(|line: &str| {
    emfor_daemon::plugin::host::fuzz_line(line);
});
