## Why

接舊式網管（電信機房的 NMS、rsyslog 到 SIEM）需要機器讀得懂的事件。emfor 的 log 只有文字，而且連 `docs/compatibility.md` 都寫「Log lines：不穩定」，所以腳本只能比對文字。狀態變化與 reload 稽核其實已經是結構化欄位，只差一種機器可讀的輸出，和一份「哪些欄位保證不變」的承諾。SNMP 與狀態變化 hook 則不內建（原因寫在 design），改在手冊示範怎麼用現有的 `emfor status --json` 接上。

## What Changes

- `[log]` 新增 `format`：`"text"`（預設，與現在相同）或 `"json"`（每行一個 JSON 物件，欄位攤平在最上層）。重啟才生效：reload 改 `format` 會像其他只能重啟的設定一樣被拒絕。
- 狀態變化那一行新增固定識別欄位 `event = "backend_state_changed"`，並承諾 `service`、`backend`、`from`、`to`、`reason`、`event` 與 JSON 的 `timestamp`、`level` 為穩定欄位；reload 稽核行本來就有 `change`，不重複加。文字模式的輸出只多一個 ` event=backend_state_changed` 欄位，措辭不變。
- `tracing-subscriber` 開 `json` feature，`Cargo.lock` 與 `fuzz/Cargo.lock` 都更新。
- 手冊：第 12 章寫 JSON log 與穩定欄位表，並補 `emfor status --json` 的 `history` 項目格式；第 14 章新增「接舊式網管（SNMP、hook）」一節，說明沒有內建 SNMP 與 hook 的原因，示範用 `snmptrap` 搭配輪詢 `emfor status --json`，以及 net-snmp 的 `extend`；第 15 章與 `docs/compatibility.md` 把「log 不穩定」改成「除下列欄位外不穩定」；第 5、18 章加 `format`；zh_TW 對應與 CHANGELOG。

## Non-Goals

- 內建 SNMP（trap 或 agent）、內建狀態變化 hook、log 輪替、把其他 log 行也加上 `event`、改現有欄位名稱。
- 讓讀設定檔之前的 log 行（plugin 啟動、設定錯誤）也是 JSON：那時還不知道 `format`。

## Capabilities

### New Capabilities

### Modified Capabilities

- `service-config`: 新增 requirement「Log format」：`[log] format` 的值、驗證、只能重啟、JSON 行的形狀。
- `health-checking`: 「Logging of state changes」加入固定欄位 `event`，並規定 JSON 模式下這一行的欄位。

## Impact

- `crates/emfor-daemon/src/config.rs`（`LogConfig`、驗證）、`crates/emfor-daemon/src/cli.rs`（logging 初始化、啟動後套用、reload 檢查）、`crates/emfor-daemon/src/schedule.rs`（狀態變化行）、`crates/emfor-daemon/Cargo.toml`、`Cargo.lock`、`fuzz/Cargo.lock`。
- 手冊第 5、12、14、15、18 章（英文與 zh_TW）、`docs/compatibility.md`、`CHANGELOG.md`。
- 新增 crate：`tracing-serde`（`tracing-subscriber` 的 `json` feature 帶入）。
