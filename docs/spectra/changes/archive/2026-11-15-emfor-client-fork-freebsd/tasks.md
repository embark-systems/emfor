## 1. 先寫會失敗的測試

- [x] 1.1 依 spec "Clients are safe across threads and fork" 的情境 "Fork while other threads look up through the C library"，新增 C API 的 fork 測試：新增 `clients/c/tests/test_fork_threads.c`，4 個 pthread 在迴圈裡呼叫 `emfor_lookup`，主執行緒 fork 300 次，每個子行程呼叫一次 `emfor_lookup` 後 `_exit`（成功為 0），父行程對每個子行程最多等 5 秒、逾時就 `SIGKILL` 並讓測試失敗；`clients/c/Makefile` 的 `TESTS` 加入 `test_fork_threads`。驗證：Debian 映像 `emfor-clients-test` 內 `make -C clients test` 通過；在 FreeBSD VM（`emfor-freebsd`，修正之前）跑同一個測試會失敗，記下卡住的是第幾個子行程。

## 2. Fork guard

- [x] 2.1 在 `clients/rust` 實作 Fork guard takes the store lock in the prepare handler 與 Forking flag closes the first-lookup gap：`FORKING` 旗標、thread-local 的寫鎖 guard、`fork_prepare` 與 `fork_release`、`Client` 取 `env` 寫鎖的方法、`Client::open` 在碰 LMDB 前等旗標，`testing` feature 底下的 `testing::fork_prepare()` 與 `testing::fork_release()`；逐一核對 `store.rs` 的 `ffi::mdb_*` 呼叫點都在 `Client.env` 的鎖底下，並把這個不變式寫在 `Client.env` 的註解。驗證：新增 `clients/rust/tests/fork_guard.rs`，Debian 映像內 `cargo test -p emfor-client --features testing --test fork_guard` 通過三個測試：`prepare_blocks_lookups_until_release`（prepare 之後另一執行緒的 lookup 等到 release 才回傳）、`release_leaves_no_hold`（prepare 與 release 重複 1000 次、期間 4 個執行緒 lookup，結束後 lookup 照常）、`forking_flag_blocks_open`（`GLOBAL` 還空時 prepare，另一執行緒的第一次 lookup 等到 release 才回傳位址）。
- [x] 2.2 [after: 1.1, 2.1] 依 Register on FreeBSD only 註冊：`global()` 第一次呼叫時用 `Once` 呼叫 `pthread_atfork`，只在 `target_os = "freebsd"`；`clients/rust/Cargo.toml` 的 `libc` 從 dev-dependency 改為一般 dependency，`Cargo.lock` 隨之更新（`clients/rust/tests/versions.rs` 與 `same_lmdb.rs` 通過、`examples/freebsd/Makefile.crates` 不變）。驗證：Debian 映像內 `cargo test -p emfor-client --features testing` 全部通過；FreeBSD VM 上 `cargo test -p emfor-client --test fork_threads` 連續 20 次通過、任務 1.1 的 C 測試通過；VM 上以 lldb 對 `fork_prepare` 與 `__thr_pshared_atfork_pre` 下中斷點，確認 `fork_prepare` 先被呼叫，結果記進 commit message。
- [x] 2.3 [after: 2.2] 依 libemfor is not unloadable on FreeBSD 確認 `dlclose`：在 FreeBSD VM 寫一個一次性的 C 程式（不進 repo），`dlopen` libemfor、lookup、`dlclose`、再 `fork`，看父與子行程是否正常結束。若 handler 留下而當機，新增 `clients/rust/build.rs` 在 FreeBSD 輸出 `cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete`，同一個程式之後不再當機；若沒有問題就不加。驗證：結果（兩種情形之一）與使用的 FreeBSD 版本記進 commit message；加了 `build.rs` 時，`cargo build` 在 Debian 映像內仍通過。

## 3. 量測

- [x] 3.1 [after: 2.2] 在 FreeBSD VM 量吞吐量：用一個一次性的程式（不進 repo）讓 1、4、8 個執行緒各連續 lookup 10 秒，修正前（`git stash` 掉 guard 的註冊，或用 HEAD 的建置）與修正後各量 5 次，記下 lookups/秒的中位數。驗證：修正後的中位數不低於修正前的 95%（95% 是這個 change 訂的容忍度，不是量測值）；數字與機器（核心數、FreeBSD 版本）記進 commit message，標明是量測值；低於容忍度時停下來回報，不自行調整門檻。

## 4. 文件

- [x] 4.1 [after: 2.3] 手冊 `docs/manual/06-clients.md` 與 `docs/manual.zh_TW/06-clients.md` 的 "Threads, processes and fork" 說明 fork 的行為與設計的 Known limits（只有 `GLOBAL` client 受保護）：FreeBSD 上 fork 會等進行中的 lookup；從 signal handler 呼叫 fork、有執行緒在 lookup 中被 `SIGSTOP`，fork 會等到它回來；`testing::use_store` 以外的 client 都受保護。`CHANGELOG.md` 的 Unreleased 把「fork 後的子行程不再卡在第一次 lookup」那一條改寫成：此前在 FreeBSD 上不成立（libthr 的 process-shared mutex 表在子行程裡留著被鎖住的狀態），現在 client 在 fork 時等到沒有 lookup 在 store 裡；不寫 0.4.0 一定有這個問題。驗證：`grep -n "FreeBSD" ` 在兩份手冊與 CHANGELOG 該條都找得到；人工比對文件與 spec 的限制一致。

## 5. 檢查

- [x] 5.1 [after: 2.3, 3.1, 4.1] 在 Debian 映像 `emfor-clients-test` 跑完整檢查：`cargo fmt --all --check`（含 `fuzz/`）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`--cfg fuzzing` 的 clippy、`cargo test --workspace --all-features`、`make -C clients test`；再在 FreeBSD VM 跑 `cargo test --workspace --all-features` 與 `tests/resolver/freebsd.sh`。驗證：全部通過，記下兩邊通過的測試數；`fork_threads` 在 VM 上不再失敗。

## 6. 獨立審查後的修正

來源：Fable 對 libthr 與修法的獨立審查（2026-11-15，報告在 scratchpad 的 `fable-src/` 與 `fable-libthr/`）。取代 design "Known limits" 裡「從 signal handler 呼叫 fork 會等待」的說法（實測是必定死結），並補上 `Once` 註冊與 fork 同時發生的競爭。

- [x] 6.1 依 design "In-store marker lets a signal handler fork" 實作：thread-local 的 `InStore` 標記（`clients/rust/src/fork.rs`），`Client::snapshot` 的讀鎖與寫鎖區段、`reopen_if_replaced` 的讀鎖與寫鎖區段都在標記之內（標記在取鎖之前設、放鎖之後清）；`prepare` 看到本執行緒在標記內就不取寫鎖（退回修正前的行為，不死結）。驗證：新增 `clients/rust/tests/fork_guard_signal.rs`，主執行緒迴圈 lookup、另一執行緒每 2 毫秒用 `pthread_kill` 送 `SIGALRM`，handler 內呼叫 `fork_prepare` 與 `fork_release`（不做真的 `fork`：被中斷的執行緒可能正在 `malloc`，真的 fork 會死結在配置器自己的 fork 鎖上，與 client 無關），3 秒內每次都回傳且至少發生一次，Debian 映像與 FreeBSD VM 都通過；沒有標記時同一個測試會死結（已實測：以 30 秒的看門狗 abort 判定）。FreeBSD 上真的從 signal handler fork 的端對端行為由 6.5 的壓力測試涵蓋。
- [x] 6.2 [after: 6.1] 依 design "Registration is per process" 把 `fork::register` 的 `Once` 改成含 pid 的狀態：fork 發生在註冊途中時，子行程看到的是別的 pid 的「註冊中」，會自己重新註冊；同一行程內另一執行緒正在註冊時，等它完成；子行程的 child handler 把狀態設成自己的 pid 與「完成」，所以不重複註冊。驗證：新增 `clients/rust/tests/fork_guard_register.rs`（`testing` 底下的 `fork_register_probe`）：狀態是別的 pid 的「註冊中」時，`register` 在 5 秒內回傳並呼叫註冊函式一次；兩個執行緒同時呼叫、註冊函式睡 100 毫秒時，註冊函式只被呼叫一次且兩個呼叫都在它完成後才回傳；完成之後再呼叫不會再註冊。
- [x] 6.3 [after: 6.1] `clients/rust/tests/fork_threads.rs` 的 lookup 執行緒數改為 `max(4, available_parallelism())`，`clients/c/tests/test_fork_threads.c` 改為 `max(4, sysconf(_SC_NPROCESSORS_ONLN))`（上限 64），避免核心數較多的機器上修正前也可能通過。驗證：Debian 映像與 FreeBSD VM 上兩個測試通過；VM 上在無 guard 的建置（`/data/prefix-before`）仍失敗。
- [x] 6.4 [after: 6.1, 6.2] 文件與 artifact：`fork.rs` 的模組註解補上父行程死結（libthr 先取 malloc 鎖再取 `pshared_lock` 讀鎖，而 `pshared_insert` 持寫鎖時呼叫 malloc）與讀鎖 prefer-reader 的說明；design 新增 "In-store marker lets a signal handler fork" 與 "Registration is per process"，更新 Known limits；spec 新增情境 "Fork from a signal handler during a lookup"；手冊（en、zh_TW）與 README（en、zh-TW）把 signal handler 內 fork 的說法改成「沒有保護，子行程可能卡住，但父行程不死結」，並說明 fork 期間所有 lookup 會等待、靜態連結 `libemfor.a` 需自行加 `-z nodelete`、行程內其他 process-shared mutex 的使用者不受保護；CHANGELOG 同步。驗證：`spectra validate emfor-client-fork-freebsd` 通過；`grep` 兩份手冊與兩份 README 都沒有「signal handler …會死結／會等待」的舊說法。
- [x] 6.5 [after: 6.3, 6.4] 完整檢查：Debian 映像 `emfor-clients-test`（fmt 含 `fuzz/`、clippy 含預設 feature 與 `--cfg fuzzing`、`cargo test --workspace --all-features`、`make -C clients test`）與 FreeBSD VM（`cargo test --workspace --all-features`、`tests/resolver/freebsd.sh`、`fork_threads` 20 次、C 測試）；另以 Fable 的壓力程式（`stress.c`，修正前後各一次）在 VM 上重跑。驗證：全部通過，修正後的壓力測試卡住的子行程為 0，記下通過的測試數。

