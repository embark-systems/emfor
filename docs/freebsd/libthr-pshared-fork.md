# FreeBSD libthr: fork() with process-shared mutexes in other threads

Status: the patch was built and tested on one system, FreeBSD 15.1-RELEASE
(arm64, a VM with 4 cores), and fixes both problems there; see
[Results](#results). It has **not been reported to FreeBSD yet**, and it was
not tried on 14.x, 13.x, amd64 or main. Measured and read-from-source
statements are marked.

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
| lock and unlock an existing process-shared mutex in a loop | maps the file again, then locks | 36 of 300 (41 to 61 in five later runs, see [Results](#results)) |
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

## Results

Tested on FreeBSD 15.1-RELEASE-p6, arm64, a VM with 4 cores. libthr was built
from the source of 15.1-RELEASE (`src.txz`; the base system is p6) twice, once
unpatched (the control) and once with this patch (`git apply`, one hunk with an
offset of 70 lines in `thr_pshared.c`), and each program ran against it through
`LD_LIBRARY_PATH`, without installing it. The repro is
`libthr-pshared-fork-repro.c` (4 threads unless noted).

| Test | libthr in the base system | Built from source, unpatched | Built from source, patched |
| --- | --- | --- | --- |
| `r new`, 300 forks, 5 runs: hung children per run | 41, 42, 41, 47, 61 | 40, 45, 53, 47, 55 | 0, 0, 0, 0, 0 |
| `r new`, 8 threads, 600 forks, 3 runs | not run | not run | 0, 0, 0 |
| `r old` and `n new` (controls) | not run | not run | 0 and 0 |
| `w new`, 50 forks, 60 s limit: the parent finishes | not run | 0 of 3 runs (killed at the limit) | 5 of 5 runs, 0 hung |
| `w new`, 8 threads, 100 forks, 3 runs | not run | not run | finishes, 0 hung |
| LMDB client (emfor's C test: 300 forks while other threads look up, no atfork workaround in the client) | hangs in 3 of 3 runs (children 1, 7, 3) | not run | passes 5 of 5 runs |
| the same client, 3 stress scenarios (900 and 1000 forks, the store replaced meanwhile) | not run | not run | 0 hung children in all three |

The client with its own atfork workaround (emfor's current client) also passes
on the patched libthr: its C test 3 of 3 and its Rust fork tests 10 of 10, so
the two do not get in each other's way.

emfor 0.4.0 (the client without the workaround), built from its tag and run
with the same fork tests on the same VM and the base system's libthr, hangs
too: its Rust test failed in 10 of 10 runs (a child hung at forks 4, 102, 20,
8, 37, 14, 43, 57, 36 and 20) and its C test in 5 of 5.

Not tested: freebsd-src `main`, 14.x, 13.x, amd64 or other architectures; the
libthr test suite; the patched library installed in the base system for other
programs. The base system is p6 and the source is 15.1-RELEASE: the repro
behaves the same on both unpatched libraries (41 to 61 and 40 to 55 hung
children per 300), but the two were not compared line by line.

## Repeating it

1. Get the source of the release (the `lib/libthr`, `lib/libc/include`,
   `lib/libc/<arch>`, `lib/libsys`, `lib/libthread_db`, `libexec/rtld-elf`,
   `share/mk` and `tools/build` directories are enough), apply the patch with
   `git apply` in the top directory.
2. Build libthr, for example
   `MAKEOBJDIRPREFIX=/tmp/obj SRCTOP=$PWD WITHOUT_TESTS=yes make -m $PWD/share/mk -C lib/libthr obj all`
   (the release's own `/usr/share/mk` has no `src.opts.mk`, hence `-m`).
3. `cc -O2 -o repro libthr-pshared-fork-repro.c -lpthread`, then run it with
   `LD_LIBRARY_PATH` set to the directory of the built `libthr.so.3`:
   `./repro /tmp/m.dat r new 4 300` (`hung` above 0 unpatched, 0 patched) and
   `timeout 60 ./repro /tmp/m.dat w new 4 50` (unpatched it does not finish:
   `procstat -t` shows the forking thread in `urdlck`; patched it finishes).
   `r old` and `n new` stay at 0 either way.
4. On other releases and architectures the code is the same and only the hit
   rate depends on cores and timing; run enough forks.

The emfor client works around both problems without a libthr change
(`clients/rust/src/fork.rs`); a fixed libthr would also help every other
program that forks next to process-shared mutexes.
