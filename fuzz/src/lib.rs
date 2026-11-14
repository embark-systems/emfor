//! What the fuzz targets share.

use std::sync::OnceLock;

/// Stack size of the threads emfor parses input on: tokio's worker and
/// `spawn_blocking` threads get 2 MiB, a quarter of the 8 MiB main thread
/// libFuzzer runs the targets on.
pub const WORKER_STACK: usize = 2 * 1024 * 1024;

/// The stack size of [`on_worker_stack`]: `EMFOR_FUZZ_STACK` (bytes) when
/// set, else [`WORKER_STACK`].
fn stack_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| match std::env::var("EMFOR_FUZZ_STACK") {
        Ok(v) => v
            .parse()
            .unwrap_or_else(|_| panic!("EMFOR_FUZZ_STACK={v} is not a number of bytes")),
        Err(_) => WORKER_STACK,
    })
}

/// Run `f` on a thread with the stack of a tokio worker, so that a
/// recursion too deep for the daemon or a plugin crashes here too (on the
/// main thread it would have four times the room). A panic or a stack
/// overflow on that thread aborts the process, which libFuzzer reports as
/// a crash.
pub fn on_worker_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(stack_size())
            .spawn_scoped(s, f)
            .expect("the system can start one more thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}
