## Context

2026-11-16 三個 Fable 子代理（模型 fable，唯讀）分別審 daemon 核心、格式與 client、檢查器與 peers。我逐項讀程式碼核對後，修了兩個 Medium 與多數 Low；之後 `code-review`（high）再審這批修正並由我核對，發現的問題也修了（見下）。以下事實都是讀原始碼、量測或測試的結果：

- `config.rs` 原本對片段與目錄只檢查 `mode & 0o022`，沒有擁有者檢查（grep `uid` 沒有結果）。符號連結被明確支援，所以連到一般使用者擁有、mode 0644 的檔案可以通過，該使用者能加 `[plugins.x] command`。
- `plugin/host.rs` 的 `PendingGuard::drop` 對任何被丟棄且仍在等待的請求都計為 unanswered，被 `abort` 的 backend 任務（reload 或停止）也算；3 個就達到 `STALL_UNANSWERED`，supervisor 會 SIGKILL 這個 plugin。
- `checker/mod.rs` 的 `run_check` 以 `req.timeout` 包住 `checker.check`，逾時就丟棄那個 future；plugin 的請求用的是同一個 `req.timeout`。
- JSON 序列化一個控制字元用 6 位元組（`\u0001`）：12 筆、key 與 message 各 512 個控制字元的 `validate` 回答是 74,105 位元組，超過 `plugin/host.rs` 的 `LINE_LIMIT`（65,536）。
- `systemd` unit 有 `User=emfor`，`ExecStartPre` 與 `ExecReload` 的 `check-config` 以 `emfor` 身分執行；FreeBSD 的 `configtest` 也以 `emfor_user` 執行。

## Goals / Non-Goals

**Goals:** 修正 review 發現且核對屬實的問題；讓規格與程式一致；保持所有既有輸出不變。

**Non-Goals:** 見 proposal。

## Decisions

### 擁有者與模式一起判斷，對已開啟的檔案做

純函式 `untrusted(mode, uid, euid)`：group 或其他人可寫（`mode & 0o022`）或擁有者不是 root 也不是 `euid` 就回傳原因。片段用 `File::open` 開啟後對 `fstat` 的結果檢查，再從同一個檔案讀；符號連結因此看的是它指向的檔案。目錄用 `metadata`（跟隨連結）檢查。`euid` 用 `nix::unistd::geteuid()`（`nix` 加上 `user` feature，`Cargo.lock` 不變）。

為什麼接受「擁有者等於 euid」：daemon 以 `emfor` 執行、檔案屬於 `emfor` 時，只有 `emfor` 本人與 root 能改它，和 root 擁有的檔案同等可信。代價：以 root 手動執行 `check-config` 時，euid 是 0，只接受 root 擁有的檔案；附的 systemd unit 與 FreeBSD `configtest` 都以 daemon 使用者執行，所以不受影響（手冊已寫）。

### 請求只有等滿它的等待時間才算「不回答」

`PluginHost::call_waiting(wait, build)`：guard 記下建立時間與 `wait`（減去時鐘誤差：20 毫秒與 `wait` 一半中較小的那個，因為最短的檢查逾時是 10 毫秒，減到 0 會讓每個被取消的請求都計入），丟棄時 `elapsed >= wait` 才計入 unanswered。`PluginChecker::check` 傳 `req.timeout`，`validate` 傳 `VALIDATE_TIMEOUT`。舊的 `call`（零等待，每次丟棄都計入）留給測試，標為 `#[doc(hidden)]`。

替代方案：讓請求自己計時，逾時才標記放棄。拒絕：外層 `run_check` 用同一個逾時，兩個計時器同時到期時，外層若先觸發就丟棄 future 而沒有標記，真正卡住的 plugin 永遠不會被偵測到，比現在更糟。已知殘餘：一個已經等滿逾時、但外層逾時還沒被輪詢到的請求若剛好被 reload 中止，仍會被計入；視窗很窄，而且那時 plugin 確實已超過逾時。

### `validate` 回答以位元組為準

`emfor-plugin` 先截斷到 12 筆、每個 key 與 message 以 `cap_reason` 限制 512 個字元，再序列化整行，超過 48 KiB 就從後面丟棄錯誤（至少留一筆）。daemon 的上限是 64 KiB，超過會停止讀取並殺掉 plugin；48 KiB 留有餘地。

### 輸出寫入失敗回報而不是 panic

`write_stdout(text, fail)`：寫到鎖定的標準輸出，`BrokenPipe` 不算失敗（與 `emfor status` 一致），其他錯誤印 `cannot write the output: …` 並回傳 `fail`。`check-config` 用 1，`lookup` 用 3（store 或 I/O 失敗；2 留給「查無服務或用法錯誤」）。

### 小修正

- `check-config --show` 的註解把控制字元寫成 `\uXXXX`，檔名不能再結束註解並注入 TOML。
- peers `secret_file`：除了「其他人不可讀」，也要「group 與其他人不可寫」（`mode & 0o022`）。
- 收到的 peer `down` 原因取前 256 個字元並 `escape_debug` 後才寫進日誌。
- `boot` 只接受 `0-9a-f`（程式原本接受大寫，與規格不符）。
- `meta/peers` 在 `peer_state` 時移除佇列中較舊的同鍵 `Put`，提交失敗重排時也以最新的為準。
- DNS 速率限制器：表滿時，同一秒最多掃描一次（`swept` 記錄上次掃描的秒）。
- Rust client：`Client::new` 改為 `pub(crate)`；`Client::for_store` 只在 `daemon-internal` feature 下編譯，`emfor-daemon` 啟用它，`libemfor`（cargo-c 以 `capi` 建置）與預設建置沒有。

## Implementation Contract

**Behavior**

- 片段或目錄：group 或其他人可寫、或擁有者不是 root 也不是 euid，載入失敗，錯誤指出路徑與原因；符號連結以目標檔案判斷；主設定檔不加檢查。
- 被取消（等待時間未到就丟棄）的 plugin 請求不計入「不回答」；等滿等待時間才丟棄的才計入。
- `validate` 回答的 JSON 行不超過 48 KiB，且至少含一筆（有錯誤時）。
- `check-config` 寫不出輸出時結束碼 1；`lookup` 為 3；管線被關掉兩者都不算失敗。
- `secret_file` group 或其他人可寫時，設定驗證失敗。
- 不使用 `--show`、沒有 `drain`、沒有新設定時，所有輸出與現在相同。

**Interface / data shape**

- `PluginHost::call_waiting(Duration, impl FnOnce(u64) -> Request)`；`emfor_client::Client::for_store` 在 `daemon-internal` feature 下才存在。

**Failure modes**

- 擁有者不合：`<path>: owned by uid N, not root or the user running emfor (uid E); refusing to load it`；可寫：`<path>: writable by group or others (mode 0NNN); refusing to load it`。
- `secret_file` 可寫：`<path> must not be writable by group or other users (chmod go-w)`。

**Acceptance criteria**

- 單元與整合測試：`owner_must_be_root_or_the_daemon_user`、`symlink_to_a_group_writable_file_is_refused`、`foreign_owner_is_refused`（root 才實際執行）、`aborted_requests_do_not_restart_the_plugin`、`validate_answer_is_bounded`、`validate_answer_is_bounded_in_bytes`、`check_config_reports_a_full_disk`（Linux）、`show_escapes_control_characters_in_comments`、`writable_peer_secret_is_refused`、`boot_is_lowercase_hex`、`full_table_is_swept_once_a_second`。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 在 Debian docker（`tools/docker-test.sh`）通過；`tools/check-manual-links.py` 通過；macOS 與 FreeBSD 發版前再測。

**Scope boundaries**

- 範圍內：上列程式、規格與文件。
- 範圍外：見 proposal 的 Non-Goals。

## Risks / Trade-offs

- **以 root 手動執行 `check-config`**：只接受 root 擁有的檔案，與 daemon（以 `emfor` 執行）的判斷不同。手冊說明；附的 unit 不受影響。
- **跨使用者的擁有者檢查只在 root 下才能測**（要 `chown`）：`foreign_owner_is_refused` 非 root 時略過，在 Debian 容器實際執行。
- **peers `secret_file` 的新檢查**會拒絕原本 group 或其他人可寫的密碼檔；文件一直寫 `chmod 600` 或 `640`。
- **RADIUS 等 `*_file` 沒有權限檢查**：刻意保留，見 Non-Goals。
