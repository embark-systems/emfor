## Why

`[peers] accept_plaintext` 預設 `true`：即使 `encrypt = true`（預設，0.4.0 起），daemon 仍接受只簽名、沒加密的 `EMP1` 訊息，只記一行 warn。擁有密鑰的人（或能在網路上偽造 `EMP1` 的人，只要拿到密鑰）因此總能讓一個預設設定的 daemon 讀取不加密的訊息，加密的保護被預設值打了折扣。TODO 67 第 12 項（CASA 自我評估的補強清單）把這個預設值列為要收緊的項目。`encrypt` 預設已是 `true`，所以只用預設值的 `[peers]` 現在都送加密訊息，收緊的時機已經成熟。

## What Changes

- `[peers] accept_plaintext` 的預設值跟著 `encrypt` 走：`encrypt` 為 `true`（預設）時預設 `false`；`encrypt = false` 時預設 `true`。明確寫出的 `accept_plaintext` 仍然優先。
- 驗證規則不變：`encrypt = false` 且 `accept_plaintext = false` 仍是 `peers.accept_plaintext` 的錯誤。因為預設值從不在 `encrypt = false` 時變成 `false`，所以沒有任何現在合法的設定會變成不合法。
- **BREAKING**（行為，不是設定格式）：預設設定的成員（`encrypt` 與 `accept_plaintext` 都沒寫）現在丟棄 `EMP1` 訊息，計入 `emfor_peer_messages_rejected_total{reason="plaintext"}`，送出 `EMP1` 的成員在這個成員看來不存活。以前這種成員會被接受並記 `warn`。影響的是還有成員送明文的群組：設了 `encrypt = false` 的成員，或 0.3.0 的成員。全部成員都已送加密訊息的群組（含從 0.4.0 以後版本升級、沒有設 `encrypt = false` 的群組）不受影響。
- peer-group spec 修改「`[peers]` 設定」要求中 `accept_plaintext` 的預設值；情境「Mixed formats during an upgrade」改成接收端明確寫 `accept_plaintext = true` 才接受明文；範例表中「`EMP1` heartbeat ... at a receiver with `encrypt = true`」的列區分接收端有沒有明確的 `accept_plaintext = true`；新增情境說明預設值如何跟著 `encrypt` 走。
- 手冊（英文與繁體中文）、`docs/deploy/daemon.md`、`examples/emfor.toml` 的註解與 CHANGELOG 的 Unreleased 寫出新預設值與升級順序：還有成員送明文時，接收端先明確設 `accept_plaintext = true`，等所有成員都送加密訊息、再刪掉這一行。
- 單元測試（`crates/emfor-daemon/src/config.rs` 中檢查 `accept_plaintext` 預設值的測試）改成涵蓋兩種 `encrypt` 值；整合測試（`crates/emfor-daemon/tests/peers.rs` 中混合格式的那一個）的接收端補上 `accept_plaintext = true`。

## Non-Goals

- 不改 `encrypt` 的預設值，也不改發送端：送出的格式仍由 `encrypt` 與每個成員最近被接受的格式決定。
- 不改驗證規則與錯誤訊息文字。
- 不改接收端的處理（`crates/emfor-daemon/src/peers/group.rs` 的 `check` 在 `accept_plaintext` 為 `false` 時回 `Reject::Plaintext`，這個行為不動），只改設定檔讀入時的預設值。
- 不處理 TODO 67 第 12 項的另一半（`[store]` 的 `file_mode` 與 `lock_file_mode`）：那個另開一個 change，因為受眾（每台應用主機）與升級方式都不同。
- 不加遷移工具或自動偵測：升級順序只寫成文件。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: `accept_plaintext` 的預設值跟著 `encrypt` 走；預設設定的成員丟棄 `EMP1`；混合格式升級的情境與範例表隨之更新。

## Impact

- Affected specs: `peer-group`（修改「`[peers]` 設定」要求、情境「Mixed formats during an upgrade」與 `EMP1` 的範例表列）
- Affected code:
  - Modified: `crates/emfor-daemon/src/config.rs`（`[peers]` 讀入時 `accept_plaintext` 的預設值，與檢查它的單元測試）
  - Modified: `crates/emfor-daemon/tests/peers.rs`（混合格式的整合測試）
  - Modified: `docs/manual/03-configuration.md`、`docs/manual.zh_TW/03-configuration.md`、`docs/manual/08-operations.md`、`docs/manual.zh_TW/08-operations.md`、`docs/deploy/daemon.md`、`examples/emfor.toml`、`CHANGELOG.md`
  - New: 無
  - Removed: 無
- 相容性：這是行為相容性變更，下一版（0.7.0）發佈；升級順序見 CHANGELOG。
