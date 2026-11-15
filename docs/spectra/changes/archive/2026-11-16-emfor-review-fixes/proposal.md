## Why

三個 Fable 子代理對整個系統做了唯讀 review（2026-11-16），沒有 High，但有 2 個 Medium 與 15 個 Low。這個 change **事後**描述已經實作並通過 Debian 閘門的修正，讓規格與程式一致，也讓它們能被 `spectra-verify` 與 `spectra-review` 檢查。review 發現的來源與核對過程在 design 裡。

## What Changes

- `conf.d` 的片段與目錄：擁有者必須是 root 或執行 emfor 的使用者，符號連結看它指向的檔案；檢查對「已開啟的檔案」做（不是檔名），讀檔也從同一個已開啟的檔案讀。
- plugin：被 reload 或停止而中止的請求，不算「呼叫者放棄等待」；`validate` 的回答以 JSON 那一行的位元組數為準，最多 12 筆錯誤、48 KiB。
- `emfor check-config` 與 `emfor lookup` 的輸出寫入失敗時回報並給結束碼（check-config 1，lookup 3），不再 panic（101）；管線被關掉不算失敗。`--show` 的註解跳脫檔名裡的控制字元。
- peers：`secret_file` 不可被 group 或其他人寫入；收到的 `down` 原因先截斷與跳脫再寫日誌；`boot` 只收小寫十六進位（程式對齊既有規格）。
- 內部：`meta/peers` 寫入在提交失敗期間只留最新一筆；DNS 速率限制器的表滿時，每秒最多掃描一次。
- Rust client：`Client::for_store` 只在 `daemon-internal` feature 下存在（`emfor lookup` 啟用，`libemfor` 與預設建置沒有）；`Client::new` 改為 crate 內部。
- 文件：手冊第 3、6 章（中英）、`docs/deploy/daemon.md`（peer 拒絕原因共七個）、CHANGELOG。

## Non-Goals

- 不替 RADIUS 的 `secret_file`、`password_file` 與 `tls_key_file` 加權限檢查（現有權限較鬆的安裝升級後會起不來；只寫進文件）。
- 不處理 peer 任務讀 `secret_file` 時卡住、Perl 對 byte string 黏著 key 的雜湊、pid 重複使用時的 fork 邊角、client 在 daemon 提交與補長度之間的微秒窗口、client 自己建立 lock 檔：觸發條件很窄，修起來風險比收益大。
- 不把 plugin 的停止偵測改成「呼叫自己計時」：那會和外層 `run_check` 的逾時互搶，外層先觸發就不計數，真正卡住的 plugin 永遠偵測不到（見 design）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: 擁有者規則；`check-config` 輸出寫入失敗的結束碼。
- `checker-plugins`: 被取消的請求不算不回答；`validate` 回答有上限。
- `peer-group`: `secret_file` 不可被 group 或其他人寫入；收到的失敗原因在寫日誌前被截斷與跳脫。
- `lookup-command`: 輸出寫入失敗時結束碼為 3。
- `client-lookup`: 命名 store 的函式不屬於公開 API。

## Impact

- 程式：`crates/emfor-daemon/src/` 的 `config.rs`、`cli.rs`、`lookup.rs`、`dns.rs`、`writer.rs`、`schedule.rs`、`plugin/host.rs`、`plugin/mod.rs`、`peers/wire.rs`、`peers/group.rs`；`crates/emfor-plugin/src/lib.rs`；`clients/rust/src/client.rs`、`clients/rust/Cargo.toml`；`crates/emfor-daemon/Cargo.toml`（`nix` 的 `user` feature、`emfor-client` 的 `daemon-internal`）。
- Spec：`docs/spectra/specs/` 下的 `service-config`、`checker-plugins`、`peer-group`、`lookup-command`、`client-lookup`。
- 文件：`docs/manual/` 與 `docs/manual.zh_TW/` 的第 3、6 章、`docs/deploy/daemon.md`、`CHANGELOG.md`。
- 相容性：`conf.d` 是今天才新增的功能，擁有者規則沒有既有安裝受影響；peers 的 `secret_file` 檢查會拒絕原本 group 或其他人可寫的密碼檔（文件寫的一直是 `chmod 600` 或 `640`）；其餘是修正，不改 store 格式、plugin 協定或 C API。Rust 公開 API：`Client` 型別是在 v0.5.0 之後（`emfor lookup --explain`）才從 `emfor-client` 匯出的（v0.5.0 的 `lib.rs` 只有 `mod client;`），所以收回 `Client::new` 與限制 `Client::for_store` 不會破壞已發佈的 API（以 `git show v0.5.0:clients/rust/src/lib.rs` 核對）。
