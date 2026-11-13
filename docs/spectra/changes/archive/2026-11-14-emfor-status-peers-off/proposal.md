## Why

emfor-peers-off-announce 之後，`meta/peers` 的每個成員帶 `services_off`（該成員宣告不參加、而本機也有的服務），但 `emfor status` 只在 `--json` 顯示它。維運人員看文字輸出時，看不出某個 backend 為什麼少一台負責主機，或為什麼沒有標 `voted`。

## What Changes

- `emfor status` 的文字輸出在 `peers: N/M live` 那一行之後、「check definition differs」那幾行之前，為每個 `services_off` 含有至少一個顯示中服務的成員加一行 `peers: <成員> leaves out <服務> ...`：只列顯示中的服務（受 `--service` 限制），依名稱排序、以空白分隔；成員依 `meta/peers` 的順序。
- 和其他 `peers:` 行一樣，只在 daemon 執行中顯示；成員的 `services_off` 缺少或為空時不顯示。
- `--json` 不變（已經有 `peers.members[].services_off`），結束碼不變。

## Non-Goals

（見 design.md 的 Goals / Non-Goals。）

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `status-command`: 「Status text output」的 `peers:` 行多一種：列出成員不參加的顯示中服務。

## Impact

- 程式：crates/emfor-daemon/src/status.rs 的 `peer_lines`。
- 測試：crates/emfor-daemon/src/status.rs 的單元測試。
- 文件：docs/manual/08-operations.md 與 docs/manual.zh_TW/08-operations.md 的聯防群組一節、CHANGELOG.md。
- 相依套件、store 格式、daemon 行為：不變。
