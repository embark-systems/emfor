## Problem

全專案審查（2026-11-13）找出三個讓整台主機的查詢失效、或讓 daemon 受 backend 影響的問題：

1. **磁碟延遲讓整台主機的 client 判定資料過期（A2）。** daemon 每秒寫一次心跳，每次 commit 都 fsync，而且直接在 async writer task 內執行。磁碟卡住超過約 2 秒（I/O 尖峰、RAID 重建、雲端磁碟節流），心跳就超過 client 預設的 `max_staleness` 3 秒，所有 client 把後端視為未知。
2. **check 的 reason 沒有長度上限、沒有過濾控制字元（A3）。** plugin 回覆、資料庫錯誤文字原樣進 store，再由 `emfor status` 印到操作員終端機：被入侵或異常的 backend 可以送入終端機 escape sequence；超長 reason 每秒重寫進 `svc/*` 與 `hist/*`，拖慢每次 lookup 的 JSON 解析。plugin 輸出一行不含換行的巨量資料時，daemon 記憶體無上限成長。
3. **daemon 重啟後數秒內服務中斷（B2）。** 重啟後所有 backend 都是 `unknown`，首次檢查延遲均勻落在 `[0, interval)`，之後要再等 `interval` 才累積到 `rise`。以預設 `interval = 2s`、`rise = 2` 計算，約 2–4 秒內 `on_all_down = "closed"` 的服務回 `ALL_DOWN`，`open` 的服務可能選到壞的 backend。

## Root Cause

1. store 以 LMDB 預設的同步模式開啟（每次 commit fsync 資料與 meta page），而 emfor 的資料其實可丟：`svc/*` 每次啟動都重寫，`hist/*` 只供 `emfor status --history`。emfor 是輔助軟體，主機當機時應用程式也一起停，fsync 保護的資料重開後也會被蓋掉。
2. reason 由各檢查器各自產生（`exec` 截 200 字元、`tcp` 的 expect 摘錄已跳脫），沒有統一的出口處理；plugin host 用 `BufReader::lines()` 讀 stdout 與 stderr，沒有行長上限。
3. 排程只有一種節奏：`unknown` 狀態也照 `interval ± jitter` 檢查。

## Proposed Solution

修改的 capability：`state-store`、`health-checking`、`checker-plugins`。

- **store 預設不 fsync**：新增 `[store] sync`（布林，預設 `false`）。`false` 以 `MDB_NOSYNC` 開啟；`true` 維持每次 commit fsync。writer 的 commit 改在 blocking 執行緒上做，不佔用 async worker。
- **損毀的 store 自動重建**：daemon 啟動時，開啟或讀取 store 遇到 LMDB 的損毀錯誤（`MDB_CORRUPTED`、`MDB_PAGE_NOTFOUND`、`MDB_INVALID`），記一筆 warn，把資料檔改名為 `<path>.corrupt`（覆蓋前一份）、刪除 `<path>-lock`，再建立新的 store。不 fsync 的 store 在主機斷電後可能損毀，這讓 daemon 能自行恢復。
- **reason 統一處理**：每個檢查結果的 reason 在寫入狀態前統一處理：tab、CR、LF 換成 `\t`、`\r`、`\n`，其他控制字元換成 `\u{XX}`，最後截到 512 個字元（超過時以 `…` 結尾）。
- **plugin 行長上限**：plugin 的 stdout 與 stderr 每行上限 65536 位元組。stdout 超過時 daemon 結束該 plugin，依既有的退出處理（待回覆請求變 `error`、退避重啟）；stderr 超過時記錄前 65536 位元組並丟棄該行其餘部分。
- **unknown 期間加快檢查**：首次檢查延遲改為 `[0, min(interval, 200ms))`；backend 狀態為 `unknown` 且上次結果為 `up` 或 `down` 時，下次檢查在 `min(interval, 200ms)` 後開始，不套 jitter。上次結果為 `error` 或狀態已確定時，照 `interval ± jitter`。`rise`、`fall` 的意義不變。

## Non-Goals (optional)

（見 design.md 的 Goals / Non-Goals。）

## Success Criteria

- 未設定 `[store] sync` 時，store 以 `MDB_NOSYNC` 開啟；`sync = true` 時不帶這個旗標（以測試讀 heed 的 env flags 確認）。
- 一個寫入垃圾 meta page 的 store 檔，daemon 啟動後記 warn、產生 `<path>.corrupt`、正常運作並寫入心跳。
- 一個 `exec` 檢查輸出含 ESC 的第一行時，store 裡的 reason 是 `\u{1b}` 形式；600 字元的 reason 在 store 裡是 512 字元並以 `…` 結尾。
- plugin 送出 70000 位元組不含換行的 stdout 時，daemon 記憶體不隨之成長、log 記錄 plugin 被結束，plugin 依退避重啟。
- `interval = "2s"`、`rise = 2`、`jitter = "0%"` 的健康 backend，在 daemon 啟動後 1 秒內變成 `healthy`（以 daemon 整合測試量測）。
- 既有測試全數通過；macOS 與 Debian 映像 `emfor-clients-test` 的 fmt、clippy、test、`make -C clients test` 通過。

## Impact

- Affected code:
  - Modified: crates/emfor-daemon/src/config.rs, crates/emfor-daemon/src/store.rs, crates/emfor-daemon/src/writer.rs, crates/emfor-daemon/src/schedule.rs, crates/emfor-daemon/src/checker/mod.rs, crates/emfor-daemon/src/plugin/host.rs, crates/emfor-daemon/tests/daemon.rs, crates/emfor-daemon/tests/plugins.rs, crates/emfor-daemon/tests/support/test_plugin.rs, examples/emfor.toml, docs/deploy/daemon.md, CHANGELOG.md
  - New: none
  - Removed: none
- 相容性：`sync` 預設由「每次 fsync」變成「不 fsync」，屬行為改變，CHANGELOG 註明；store 格式不變（`meta/schema_version` 仍為 1），client 不需更新。
