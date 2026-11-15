## Problem

在 FreeBSD 上，多執行緒的行程 fork 之後，子行程的第一次 lookup 會永遠卡住。`clients/rust/tests/fork_threads.rs`（spec "Clients are safe across threads and fork" 的情境 "Fork while other threads look up"：4 個執行緒不停 lookup，另一個執行緒 fork 300 次，每個子行程的 lookup 都要在 5 秒內回傳）在 FreeBSD 15.1-RELEASE-p6（arm64，VM）上每次執行都失敗，已量測：直接執行 5 次，加上完整測試 2 次，共 7 次，每次都是某個子行程（第一次是第 50 個）卡滿 5 秒。同一個測試在 Debian 通過。

這個修正的 CHANGELOG（Unreleased）寫「fork 後的子行程不再卡在第一次 lookup」，在 FreeBSD 上不成立。使用 libemfor 的 C、Python、PHP、Perl、Go client 走同一個 `global()`，都受影響。

0.4.0 在 FreeBSD 上很可能有同樣的問題，但沒有驗證：這個測試是 0.4.0 之後才加的，2026-11-14 對 0.4.0 的 FreeBSD 測試沒有跑它。

## Root Cause

已量測：在 VM 上以 lldb 看卡住的子行程，堆疊是 `mdb_txn_begin` → `mdb_txn_renew0` → `pthread_mutex_lock`（LMDB 放在 lock 檔裡、process-shared 的 reader 表 mutex）→ libthr 的 `__thr_pshared_offpage` → `pshared_wlock`，在 umtx 上睡在 libthr 內部的 `pshared_lock`。

libthr 靠這把 `pshared_lock` 查 process-shared mutex 的內部表。從 VM 上的 `/lib/libthr.so.3` 反組譯：libthr 自己的 fork 處理 `__thr_pshared_atfork_pre` 只做 `_thr_rwl_rdlock(&pshared_lock)`，`__thr_pshared_atfork_post` 解鎖。讀鎖不會等其他讀者，所以 fork 的那一刻，如果另一個執行緒正在 LMDB 的 mutex 操作裡（查表時持有 `pshared_lock` 的讀鎖），這個讀鎖會被子行程繼承，而那個執行緒在子行程裡已經不存在，沒有人會放掉它。子行程第一次對新映射的 lock 檔使用 process-shared mutex 時要取寫鎖，就永遠等下去。

Linux（glibc）沒有這張內部表，同一個測試通過。macOS 的 LMDB 用 POSIX semaphore，沒有測過，這裡不下結論；其他 BSD 沒有測過。

## Proposed Solution

在 client（`clients/rust`）裡讓 fork 的當下沒有任何執行緒在 LMDB 裡，用 `pthread_atfork`：

- prepare：取這個行程 client 的 store 讀寫鎖（`Client.env`）的寫鎖。client 對 LMDB 的每個呼叫本來就在這把鎖底下（lookup 在讀鎖，open、reopen、調整 map size 在寫鎖），所以取到寫鎖就代表沒有執行緒在 LMDB 裡，新的 lookup 也會被擋住。把鎖留在 thread-local，直到 parent 與 child handler 放掉（三者都在呼叫 fork 的同一個執行緒上執行）。
- 沿用現有的鎖，lookup 的快路徑沒有新增任何共用的原子變數或鎖。這是估計，要以 FreeBSD VM 上的多執行緒 lookup 吞吐量實測，修正前後比較。
- 行程第一次 lookup 與 fork 同時發生的縫隙：prepare 先設一個行程範圍的 forking 旗標（SeqCst），再讀 `GLOBAL`；`Client::open` 碰 LMDB 之前檢查旗標，設著就等；parent 與 child handler 清掉旗標。
- handler 只在 `target_os = "freebsd"` 註冊（`Once`，從第一次 `global()` 起）；程式碼各平台都編譯，單元測試直接呼叫 prepare 與 release，所以 Debian 也能測到邏輯。
- libemfor（cdylib）在 FreeBSD 上設成不可卸載（`-Wl,-z,nodelete`），避免被 `dlclose` 之後 handler 指向已卸載的程式碼；先在 VM 上確認 FreeBSD 的 `pthread_atfork` 對被 `dlclose` 的函式庫怎麼處理，如果不需要就不加。
- 已在 VM 的暫存副本上驗證過：加上這個 guard 後 `fork_threads` 連續 20 次通過（每次約 0.5 秒），修正前每次失敗；測試的執行檔也確認含有 guard。repo 裡沒有改動。

## Non-Goals (optional)

- 其他 BSD、macOS：沒有測過，不註冊 handler，之後有需要再另開 change。
- 改 LMDB、改 lock 檔格式。LMDB 的 `posix-sem` 會改變 lock 檔格式，daemon 與所有 client 必須一起換，是 breaking change，不採用。
- 改 daemon。
- 在每個 LMDB 呼叫外面包獨立的計數器或讀寫鎖：每次 lookup 多一條被各執行緒爭用的 cache line，不採用。
- `testing::use_store` 建立的 client 不受保護（只有行程的 `GLOBAL` client 受保護）；從 signal handler 在正在 lookup 的同一個執行緒上呼叫 fork，或有執行緒在 lookup 中被 SIGSTOP，fork 會等到它結束，這幾點寫進文件，不處理。

## Success Criteria

- FreeBSD VM 上，修正前 `cargo test -p emfor-client --test fork_threads` 每次失敗（已量測），修正後連續 20 次通過；C API 的同樣測試（新增）修正前失敗、修正後通過。
- FreeBSD VM 上 `cargo test --workspace --all-features` 全部通過，`tests/resolver/freebsd.sh` 通過。
- Debian 映像 `emfor-clients-test` 的完整檢查通過（fmt、clippy 含 `--cfg fuzzing`、`cargo test --workspace --all-features`、`make -C clients test`）；單元測試證明 prepare 會等進行中的 lookup、release 之後 lookup 繼續、fork 之後沒有鎖留在父或子行程裡。
- 多執行緒 lookup 吞吐量在 FreeBSD VM 上修正前後的實測值沒有變差（量測，不是估計）。
- CHANGELOG 的 Unreleased 說明先前的條目在 FreeBSD 上不成立、這次修好；不寫 0.4.0 一定會卡，除非已經驗證。

## Impact

- Affected specs: client-lookup
- Affected code:
  - Modified: clients/rust/src/lib.rs, clients/rust/src/client.rs, clients/rust/Cargo.toml, clients/c/Makefile, CHANGELOG.md, docs/manual/06-clients.md, docs/manual.zh_TW/06-clients.md
  - New: clients/c/tests/test_fork_threads.c, clients/rust/build.rs（FreeBSD 的 `nodelete`，若 VM 檢查確認需要）
  - Removed: none
