# FreeBSD libthr: fork() with process-shared mutexes in other threads

Status: a draft. The patch has **not been built or run** (a libthr build needs
the FreeBSD source tree, which the test VM does not have), and this has **not
been reported to FreeBSD yet**. What was measured and what was only read from
the source is marked below.

| File | What it is |
| --- | --- |
| [libthr-pshared-fork.patch](libthr-pshared-fork.patch) | The patch, for `lib/libthr` of freebsd-src (`git apply`) |
| [libthr-pshared-fork-repro.c](libthr-pshared-fork-repro.c) | A small program without LMDB that shows both problems |

## The problem

A `fork()` in a multithreaded process whose other threads use process-shared
mutexes has two problems. LMDB is an example: it keeps its reader mutex in the
lock file, as a process-shared mutex, and takes it on every `mdb_txn_begin`.
emfor's client library hits both ([README](../../README.md#freebsd-fork-while-other-threads-look-up)).

1. **The child can hang.** A child forked while another thread is inside a
   process-shared mutex call waits for ever the first time it maps a
   process-shared mutex it has not used (for example, an LMDB environment it
   opens again).
2. **The parent can deadlock.** A parent that forks while another thread maps
   a process-shared mutex for the first time can stop for ever inside `fork()`.

## Evidence

Measured on FreeBSD 15.1-RELEASE-p6, arm64, in a VM with 4 cores. The table
is from one run of `libthr-pshared-fork-repro.c` (4 threads, 300 forks) by an
independent review of this problem; the hung child's lock state and the
emfor test (a child hangs within the first 50 of 300 forks, in every run) are
from the author's own runs.

| Parent threads | Child | Hung children |
| --- | --- | --- |
| lock and unlock an existing process-shared mutex in a loop | maps the file again, then locks | 36 of 300 |
| the same | locks through the mapping it inherited | 0 of 300 |
| do not touch the mutex | maps the file again, then locks | 0 of 300 |

In a hung child, `pshared_lock` (read with lldb) was `rw_state = 0x40000001`
(a writer waiting, one reader left), `rw_blocked_writers = 1`, and the
thread was in `__thr_pshared_offpage()` (`pshared_wlock()`), called from
`check_and_init_mutex()` from `pthread_mutex_lock()`. In emfor the stack
continues `mdb_txn_begin` and `mdb_txn_renew0`.

The parent deadlock reproduces with the `w` mode (threads map the file, lock,
unlock and unmap in a loop): the forking thread waits in
`__thr_pshared_atfork_pre()`, another thread waits in
`pshared_insert()` to `malloc()`.

## Cause

From the source of `lib/libthr/thread/thr_fork.c` and `thr_pshared.c` (the
same in stable/13, stable/14, stable/15 and main, compared; read by a second
review back to releng/12.4):

1. `thr_fork_impl()` calls `__thr_pshared_atfork_pre()`, which is
   `_thr_rwl_rdlock(&pshared_lock)`, and the child calls
   `__thr_pshared_atfork_post()`, `_thr_rwl_unlock()`. A read lock does not wait
   for other readers, and `_thr_rwl_rdlock()` passes `URWLOCK_PREFER_READER`.
   `__thr_pshared_offpage()` takes the lock for reading to look a mutex up, on
   every lock and unlock of a process-shared mutex. A thread in that section
   at the fork leaves a read hold in the child; its thread is gone, and the
   first writer in the child (`pshared_insert()` for a new mapping) never gets
   the lock. The child's `_thr_atfork_lock` is reinitialised by
   `_libpthread_init()`, `pshared_lock` is not (`__thr_pshared_init()` runs
   once).
2. `thr_fork_impl()` takes the allocator's locks (`_malloc_prefork()`) before
   `__thr_pshared_atfork_pre()`, while `pshared_insert()` calls `malloc()`
   holding `pshared_lock` for writing: opposite orders.

Read from the source, not run: `main` caches the last lookup per thread
(`pshared_cache_lookup()`), which makes the first problem much rarer there but
not impossible (the cache is invalidated when any mapping changes).

## The patch

- `thr_fork.c`: `__thr_pshared_atfork_pre()` moves before
  `__thr_malloc_prefork()` and `_malloc_prefork()`, so the order is
  `pshared_lock` and then the allocator, as in `pshared_insert()`. The child
  calls the new function below instead of `__thr_pshared_atfork_post()`.
- `thr_pshared.c`: `__thr_pshared_atfork_child()` calls
  `_thr_urwlock_init(&pshared_lock)`. The forking thread holds the lock for
  reading across the fork, so no writer was changing the hash, which is
  consistent; and no other thread of the parent exists in the child, so no one
  can still hold the lock.
- `thr_private.h`: the declaration.

Not done on purpose: taking the lock for writing in `__thr_pshared_atfork_pre()`
(it would wait for the readers) would let a steady stream of readers, which the
lock prefers, starve the fork.

`git apply --check` passes on freebsd-src `main`, `stable/15` and `stable/14`
(`thr_pshared.c` differs between them, the changed part does not).

## Testing it (not done)

1. Build libthr from the patched source (`make -C lib/libthr` in the source
   tree) and install it, or run the repro with
   `LD_LIBRARY_PATH` set to the directory of the freshly built `libthr.so.3`.
2. `cc -O2 -o repro libthr-pshared-fork-repro.c -lpthread`.
3. `./repro /tmp/m.dat r new 4 300`: before the patch `hung` is above 0 (it
   needs enough runs and cores), after it should be 0. `r old` and `n new`
   should stay 0.
4. `./repro /tmp/m.dat w new 4 50`: before the patch the parent can stop
   (`procstat -t` shows `urdlck`; `kill -9` it); after it should finish.
5. Run it on 14.x, 13.x and amd64 too: the code is the same, and only the hit
   rate depends on cores and timing.

The emfor client works around both problems without a libthr change
(`clients/rust/src/fork.rs`); a fixed libthr would also help every other
program that forks next to process-shared mutexes.
