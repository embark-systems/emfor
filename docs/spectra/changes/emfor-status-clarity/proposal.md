## Why

值班的人被叫醒時，會用 `emfor status` 問兩件事：這台主機為什麼把某個 backend 判 down，以及流量現在往哪裡去。評估後發現文字輸出在這兩件事上各缺一塊資訊，而 peer 相關的輸出本身大致夠用：

- store 的每個 backend 記錄有 `backup` 欄位，候選者的選擇先看 primary、再看 backup（手冊第 5 章「Backup backends」），但 `emfor status` 完全不顯示它，健康的 primary 與 backup 看起來一樣，讀的人無法回答「流量為什麼去那一台」。
- 一個 backend 在連續失敗達 `fall` 次之前仍是 `healthy`，但它最近一次檢查已失敗，`reason` 已設定，而且只要還有別的 healthy backend 通過檢查，它就已被排除在候選者之外（手冊第 5 章「A backend whose checks start failing」）。畫面上只看到 `healthy` 後面接著一段 reason，讀起來像「健康、有在收流量」。
- 聯防群組裡活著的成員太少（本機加上沒有略過該服務、仍 live 的成員不到 3 個）時，沒有 assigned checker，所有 reason 都是本機自己判的，沒有 `peers d/f:` 前綴。手冊自己的範例 `peers: 1/2 live` 就是這種狀態，但輸出沒說「群組現在投不了票」。
- `docs/deploy/daemon.md` 與手冊第 8 章的標記清單漏了 `local-up` 與 `voted`，`daemon.md` 的 `--json` 欄位清單漏了 `peers` 與 `assigned_checkers`。

## What Changes

- `emfor status` 的 backend 行：`backup = true` 的 backend 顯示標記 `backup`。
- backend 行：狀態為 `healthy` 且 `reason` 有值的 backend 顯示標記 `last check failed`。措辭只陳述事實，不說「已排除」，因為「所有 healthy backend 都在失敗時，它們全部仍是候選者」這條規則不能被畫面反駁。
- `peers:` 行：當群組無法投票時，行尾加 `, too few live members to vote`。條件與 `voted` 同一套計算：能算出 assigned checker（`listen` 與成員位址有效），至少一個顯示的 backend 屬於沒有被本機略過的服務，而且這些 backend 全都沒有 assigned checker。
- 文件：`docs/deploy/daemon.md` 與中英手冊第 8 章（`docs/manual`、`docs/manual.zh_TW`）補上 `backup`、`last check failed`、`local-up`、`voted` 與新的 `peers:` 提示的說明；`local-up` 段落補一句指向 `emfor status --json` 的 `peers` 與 `assigned_checkers`；`daemon.md` 的 `--json` 欄位清單補上 `peers` 與 `assigned_checkers`。
- CHANGELOG 的 Unreleased 段落記錄以上變更。
- 本變更不打 tag，隨 0.5.0 之後的下一版發佈。

## Non-Goals

- 不改 `--json`：沒有新增、移除或改名任何欄位。`backup` 與 `reason` 早就在 `--json` 的 backend 記錄裡。
- 不改結束碼，也不把 peer 狀態放進結束碼。
- 不移除或改寫 `voted`、`checks N/M backends here`、`peers: <member> leaves out`；它們價值較低但不會誤導。
- 不加 slow start 標記，不顯示 `locality` 用的 backend site，不把 peer 的 Prometheus 指標（例如 `emfor_peer_messages_rejected_total`）搬進文字輸出；那需要新的 store 欄位。
- 不改 daemon、檢查或投票行為。
- 不做「每個服務各一行」的投不了票提示：只有全部可投票的服務都投不了時才顯示，部分服務投不了的情形留在文件裡說明，改用 `--json` 的 `assigned_checkers` 查。
- 文字輸出依 `docs/compatibility.md` 不屬於穩定介面（版面與措辭可在任何版本更動），所以這些調整不是不相容變更；仍會在 CHANGELOG 中寫明。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `status-command`: 「Status text output」要求新增 `backup` 與 `last check failed` 兩個 backend 標記，以及 `peers:` 行的「too few live members to vote」後綴，並更新各情境與範例。

## Impact

- 程式：`crates/emfor-daemon/src/status.rs`（`services_lines` 的 backend 標記、`peer_lines` 的後綴，以及該檔案裡逐字比對文字輸出的單元測試）。
- Spec：`docs/spectra/specs/status-command/spec.md`。
- 文件：`docs/deploy/daemon.md`、`docs/manual/08-operations.md`、`docs/manual.zh_TW/08-operations.md`；必要時 `docs/manual/05-backend-selection.md` 與其繁中版的交叉說明。
- `CHANGELOG.md`（Unreleased）。
- 不影響 store 格式、plugin 協定、client、DNS、metrics 與設定檔。
