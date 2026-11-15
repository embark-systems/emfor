## Context

client（`clients/rust`，libemfor 與 Go、Python、PHP、Perl client 都用它）在行程內只有一個 `GLOBAL` client（`global()`），它的 `Client.env` 是一把 `RwLock<Option<Env>>`：lookup 在讀鎖底下開 read transaction，open、reopen、調整 map size 在寫鎖底下。fork 之後子行程以 pid 不同為由丟棄繼承的 client（不關閉），建立新的 client 並重新開啟 store。

LMDB 的 reader 表放在 lock 檔裡，用 process-shared 的 pthread mutex 保護。FreeBSD 的 libthr 用一張內部表（以 `pshared_lock` 這把讀寫鎖保護）管理 process-shared mutex，`fork` 時 libthr 自己的處理 `__thr_pshared_atfork_pre` 只取這把鎖的讀鎖（已用 lldb 反組譯 `/lib/libthr.so.3` 確認），讀鎖不等其他讀者。某個執行緒在 LMDB 的 mutex 操作裡（持有讀鎖）時被 fork，子行程繼承這個讀鎖，那個執行緒卻不存在，子行程第一次對新映射的 lock 檔使用 mutex（要取寫鎖）就永遠等下去。

已量測（FreeBSD 15.1-RELEASE-p6 arm64 VM）：`fork_threads` 每次失敗（連續 5 次，加上完整測試 2 次，共 7 次），第一次失敗時是第 50 個子行程；在 VM 的暫存副本加上下面的 guard 後連續 20 次通過，300 次 fork 約 0.5 秒（量測：8 次 0.47 至 0.57 秒）。

## Goals / Non-Goals

**Goals:**

- fork 時行程內沒有執行緒在 LMDB 裡，子行程的 lookup 不會卡在繼承來的 libthr 鎖上（FreeBSD）。
- lookup 快路徑不增加共用的原子變數或鎖，吞吐量不變差（以量測確認）。
- Rust、C、Go、Python、PHP、Perl client 都受益，lock 檔格式與 daemon 不變。

**Non-Goals:**

- 其他 BSD、macOS：沒有測過，不註冊 handler；LMDB 的 `posix-sem`、改 lock 檔格式、改 daemon。
- 保護 `testing::use_store` 建立的 client；處理從 signal handler 呼叫 fork 的情形。

## Decisions

### Fork guard takes the store lock in the prepare handler

用 `pthread_atfork` 註冊三個 handler。prepare：取 `GLOBAL` client 的 `Client.env` 寫鎖，把 guard 放進 thread-local。parent 與 child：丟掉 guard。三個 handler 都在呼叫 fork 的同一個執行緒上執行，所以 thread-local 取得到同一個 guard；子行程裡其他執行緒不存在，丟掉 guard 就是解鎖子行程自己的那份記憶體。

client 對 LMDB 的每個呼叫都在 `Client.env` 的鎖底下：`snapshot` 的快路徑在讀鎖，開啟、`reopen_if_replaced`、`adopt_map_size`、關閉舊 `Env` 在寫鎖。取到寫鎖就代表沒有執行緒在 LMDB 裡，新的 lookup 也被擋在鎖外，直到 fork 在父與子都回傳。這個不變式寫在 `Client.env` 的註解裡，任務 2.1 逐一核對 `store.rs` 的 `ffi::mdb_*` 呼叫點。

沿用現有的鎖，所以 lookup 快路徑沒有新增任何東西（估計值；任務 3.1 在 FreeBSD VM 上實測）。

替代方案：(a) 在每個 LMDB 呼叫外另加行程範圍的計數器或讀寫鎖，所有 lookup 的執行緒會在同一條 cache line 上爭用，而 lookup 吞吐量本來就受 LMDB 的 reader 鎖限制，不採用。(b) 只在 Rust API 上包一層：C API、Go、Python、PHP、Perl 都經過同一個 `global()`，在這裡做就全部涵蓋，不需要另外包。(c) LMDB 的 `posix-sem` feature：不用 libthr 的 process-shared mutex，但 lock 檔格式會變，daemon 與所有 client 必須一起換，是 breaking change，不採用。

### Forking flag closes the first-lookup gap

只有 `GLOBAL` 已存在時 prepare 才取得到鎖。行程的第一次 lookup 與 fork 同時發生時，`GLOBAL` 可能還是空的，或剛存進去、store 還沒開完。處理：prepare 先把行程範圍的 `FORKING`（`AtomicUsize`，計算正在準備的 fork 數，因為兩個執行緒可以同時 fork）加一（SeqCst），再讀 `GLOBAL`；開 store 的執行緒在取得 `Client.env` 寫鎖之前（`snapshot` 的慢路徑與 `reopen_if_replaced`，兩處都會呼叫 `Env::open`）檢查 `FORKING`，不為 0 就讓出 CPU 等（`yield`）；parent handler 減一，child handler 設為 0（其他執行緒的 fork 在子行程裡不存在，不會再減）。只有走到慢路徑（開啟或換掉 store）的執行緒會等，而且只等一次 fork 的時間，所以用 `yield` 輪詢，不另做喚醒機制。等待一定在取得寫鎖之前：持有寫鎖時等旗標，會與等這把寫鎖的 prepare 互相等待而死結（實作時發現，原本寫的是在 `Client::open` 裡等，那時已持有寫鎖）。`global()` 先把 client 存進 `GLOBAL` 才開 store，所以：prepare 讀到 `GLOBAL` 時，prepare 的寫鎖會等已持有寫鎖、正在開 store 的執行緒；prepare 讀到空時，那個執行緒之後的檢查一定看到旗標（兩邊都是 SeqCst：先寫旗標再讀 `GLOBAL`，對先寫 `GLOBAL` 再讀旗標）。旗標只在 `open` 讀，平常沒有人寫，不在 lookup 快路徑上。

### Register on FreeBSD only

guard 的程式碼各平台都編譯；`pthread_atfork` 只在 `target_os = "freebsd"` 註冊，從第一次 `global()` 呼叫起用 `Once` 註冊一次。Linux 的同一個測試不會卡住，多一個 atfork handler 在那裡只有風險沒有好處。其他 BSD 沒有測過，不註冊。`clients/rust/Cargo.toml` 把 `libc` 從 dev-dependency 改成一般 dependency（`Cargo.lock` 已有 libc，`examples/freebsd/Makefile.crates` 已列出 libc-0.2.191，不需要改）。

為了在 Debian 上測邏輯，`testing` feature 底下提供 `testing::fork_prepare()` 與 `testing::fork_release()`，直接呼叫同樣的函式（不是公開 API）。

替代方案：所有 Unix 都註冊。這樣 Debian CI 會走到註冊的程式，但 `fork_threads` 在 Linux 沒有 guard 也通過，證明不了什麼，卻讓每個 Linux 行程多一個 handler。不採用。

### libemfor is not unloadable on FreeBSD

行程用 `dlclose` 卸載 libemfor 之後，留在 libc 的 atfork 表裡的 handler 會指向已卸載的程式碼，下一次 fork 就會當機。FreeBSD 的 libc 有 `pthread_atfork_exp`（與 `pthread_atfork_int`），不確定它是否依 `dlclose` 移除 handler；任務 2.3 在 VM 上用 dlopen、lookup、dlclose、fork 的小程式確認。若 handler 沒有被移除，在 FreeBSD 上用 `clients/rust/build.rs` 輸出 `cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete`，讓 `dlclose` 不卸載它；若已被移除就不加。PHP 在關閉時會卸載擴充模組，Python 的 ctypes 不會。

### In-store marker lets a signal handler fork

Supersedes "Known limits" 的「從 signal handler 呼叫 fork，prepare 會等這個執行緒自己的 lookup，互等」：Fable 實測，修正前在主執行緒迴圈 lookup、SIGALRM handler 內 fork，10 秒完成 9999 次；修正後在 `fork::prepare` 的寫鎖上必定死結，從「通常沒事」變成「必定卡死」，不可接受。

做法：thread-local 的 `InStore` 計數（不是共用變數，不增加爭用），`Client::snapshot` 與 `reopen_if_replaced` 在取 `Client.env` 的鎖之前設、放鎖之後清（所以涵蓋取鎖到放鎖的整段）。`prepare` 看到本執行緒在標記內，就不取寫鎖，只把 `FORKING` 加一（照常配對減一）：fork 這種情形退回修正前的行為（子行程可能卡住），但父行程不死結。標記設在取鎖之前，是故意的：取鎖與標記之間沒有空隙，寧可少保護一次，也不讓同一個執行緒對自己持有的讀鎖要寫鎖。

### Registration is per process

Supersedes "Register on FreeBSD only" 裡用 `Once` 註冊的做法：`call_once` 執行途中另一個執行緒 fork（`pthread_atfork` 要取 libthr 的 atfork 寫鎖，會被進行中的 fork 擋住），子行程的 `Once` 永遠停在「執行中」，子行程第一次 lookup 卡在 `register()`（Fable 推論，未重現；窗口是行程第一次 lookup 的幾百奈秒）。

做法：`REG` 是一個 `AtomicU64`，值為 `pid << 2 | 階段`（0 未註冊、1 註冊中、2 完成）。`register` 看到「自己 pid 的註冊中」就讓出 CPU 等；看到「自己 pid 的完成」就返回；其他（從未註冊，或是從父行程繼承來的別的 pid）就以 compare-exchange 搶到註冊權後自己註冊。子行程的 child handler 把 `REG` 設成自己的 pid 與「完成」，所以在註冊完成之後 fork 的子行程不會重複註冊。重複註冊也無害：`PREPARED` 讓重複的 prepare 與 release 不動作。註冊函式可注入（`register_with`），所以 Debian 上也能測狀態機。`pthread_atfork` 失敗（只可能是記憶體不足）時不 panic、不記錄為完成：狀態回到「未註冊」，下一次 lookup 再試（lookup 本身不因此失敗，沒有 handler 時行為和沒有這個 change 之前一樣）；註冊函式 panic 時也一樣（review 指出原本的 `assert_eq!` 加上「無論如何都記為完成」會讓被攔下的 panic 之後永遠沒有 handler）。

### Known limits

寫進手冊，不處理：

- 只有行程的 `GLOBAL` client 受保護；`testing::use_store` 建立的 client（只用於測試）不受保護。
- 從 signal handler 呼叫 fork，而被中斷的正是正在 lookup 的同一個執行緒：沒有保護（子行程可能卡住，和修正前一樣），但父行程不會死結（見 "In-store marker lets a signal handler fork"）。lookup 本身不是 async-signal-safe，這種用法本來就不被支援。
- 有執行緒在 lookup 中被 `SIGSTOP`，或卡在 LMDB 裡很久，fork 會等到它回來；反方向也一樣：fork 進行期間，所有 lookup 都等到 fork 在父與子都回傳。
- 靜態連結 `libemfor.a` 到會被卸載的模組，或自建的 Rust cdylib 依賴這個 crate：`-z nodelete` 只作用在 `libemfor.so`，要自行加。
- 行程內其他使用 process-shared mutex 的程式（另一個 LMDB environment 等）不受保護，只能等 FreeBSD 修。
- 另一個 libthr 問題：`fork` 先取 malloc 鎖再取 `pshared_lock` 的讀鎖，而 `pshared_insert` 持有寫鎖時呼叫 malloc，所以父行程在別的執行緒第一次映射 process-shared mutex 的當下 fork 可能自己死結（Fable 用不含 LMDB 的程式實測重現）。這個 guard 也擋住它，因為 emfor 開啟或重開 store 一律在 `Client.env` 寫鎖裡，prepare 會等；讀鎖是 prefer-reader（`URWLOCK_PREFER_READER`），所以也不能只在 libthr 端的鎖上等。
- prepare 的先後：已用 lldb 在 VM 上確認：user handler 的 `fork::prepare`（從 libthr 的 `thr_fork_impl` 呼叫）先於 `__thr_pshared_atfork_pre`，`fork::release` 最後。順序相反的話，需要 libthr 寫鎖的 lookup 會與 fork 互等。

## Implementation Contract

**Observable behavior**

- FreeBSD 上，有多個執行緒在 lookup 的行程 fork 之後，每個子行程的第一次 lookup（Rust API 與 C API）在 5 秒內回傳有效位址（spec 情境 "Fork while other threads look up"，以及新增的 C library 與 "Fork during the first lookup" 情境）。
- 父行程的 lookup 在 fork 之後照常回傳；fork 期間被擋住的 lookup 在 fork 回傳後繼續。
- Linux 與其他平台的行為不變（不註冊 handler）。

**Interface**

- `clients/rust/src/lib.rs`（或新的 `fork.rs`）：`FORKING: AtomicUsize`（正在準備的 fork 數）；thread-local 持有 `RwLockWriteGuard<'static, Option<Env>>`，另有 `PREPARED` 與 `IN_STORE`（`InStore` 標記，見 "In-store marker lets a signal handler fork"）；`REG: AtomicU64`（註冊狀態，見 "Registration is per process"）；`register_with`；`extern "C" fn prepare()`、`release()`、`release_child()`；`global()` 在第一次呼叫時 `Once` 註冊（只在 `target_os = "freebsd"`）。
- `clients/rust/src/client.rs`：`Client::lock_store` 取 `env` 寫鎖；`snapshot` 的慢路徑與 `reopen_if_replaced` 在取得寫鎖前呼叫 `fork::wait_while_forking`。
- `clients/rust/src/fork.rs`（新）：上述計數、guard 與 handler（prepare、parent 的 `release`、child 的 `release_child`；thread-local 的 `PREPARED` 讓重複的 prepare 與沒有配對的 release 都不動作）；`clients/rust/build.rs`（新）：FreeBSD 上輸出 `-z nodelete`（任務 2.3 的 VM 檢查確認 `dlclose` 之後 fork 會當機，已加）。
- `testing::fork_prepare()`、`testing::fork_release()`、`testing::fork_release_child()`（`testing` feature，非公開 API）。
- `clients/c/tests/test_fork_threads.c`、`clients/c/Makefile` 的 `TESTS` 增加 `test_fork_threads`。

**Acceptance criteria**

- `clients/rust/tests/fork_guard.rs`（Debian 與 VM 都跑）：`prepare_blocks_lookups_until_release`、`release_leaves_no_hold`（prepare 與 release 重複 1000 次，期間 4 個執行緒 lookup，結束後 lookup 照常）；`clients/rust/tests/fork_guard_first.rs`：`forking_flag_blocks_open`（`GLOBAL` 還空時 prepare 並真的 fork，子行程與父行程的第一次 lookup 都回傳位址；行程的 client 只能建立一次，所以是獨立的測試檔）；`clients/rust/tests/fork_guard_concurrent.rs`：`one_release_does_not_end_another_fork`（兩個執行緒同時準備 fork，一個 release 之後另一個 fork 還在準備，第一次 lookup 仍要等）；`fork_guard.rs` 另有 `prepare_twice_needs_one_release`。
- FreeBSD VM：`cargo test -p emfor-client --test fork_threads` 修正前失敗（已量測）、修正後連續 20 次通過；新的 C 測試修正前失敗、修正後通過；`cargo test --workspace --all-features` 與 `tests/resolver/freebsd.sh` 通過。
- 吞吐量：FreeBSD VM 上 1、4、8 個執行緒的 lookups/秒，修正前後各量 5 次，後者的中位數不低於前者的 95%（95% 是這個 change 訂的容忍度，不是量測值）。
- Debian 映像 `emfor-clients-test` 的完整檢查通過。

**In scope**：`clients/rust` 的 guard、`Cargo.toml`、（條件式）`build.rs`、C 測試、手冊（en、zh_TW）、CHANGELOG、spec client-lookup。

**Out of scope**：其他 BSD、macOS、LMDB 與 lock 檔格式、daemon、CI 的 freebsd job 目前只跑 `tests/resolver/freebsd.sh`、不跑 `fork_threads`，要不要加進去另案處理。

## Risks / Trade-offs

- [fork 要等進行中的 lookup] → lookup 每次只持有 reader slot 到讀完為止，等待時間是一次 lookup 的長度（估計值，沒有單獨量測；`fork_threads` 300 次 fork 約 0.5 秒是量測的上限參考）。
- [handler 被 `dlclose` 後指向卸載的程式] → 任務 2.3 在 VM 上確認，必要時加 `nodelete`。
- [0.4.0 在 FreeBSD 上很可能有同樣問題，但沒有驗證] → 不在文件中寫 0.4.0 一定會卡；CHANGELOG 只說這個版本修好。

## Migration Plan

不需要遷移。lock 檔格式與 daemon 不變，client 與 daemon 可以各自升級；只有 FreeBSD 上的 client 行為不同。回退就是還原這個 change，沒有資料要處理。

## Open Questions

（無）
