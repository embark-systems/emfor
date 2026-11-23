## Context

emfor 的 log 走標準錯誤，格式是 `tracing_subscriber::fmt` 的文字（`crates/emfor-daemon/src/cli.rs` 的 `init_logging`）。狀態變化行（`schedule.rs`）與 reload 稽核行（`audit.rs` 的 `log_one`）都已經帶結構化欄位，但手冊與 `docs/compatibility.md` 都說「log 行不穩定」，只有稽核行的欄位名與 `change` 值例外。要接舊式網管（NMS、rsyslog 轉 SIEM）的人沒有可以依賴的機器可讀輸出。`emfor status --json` 已經是穩定的，而且 `history` 帶每個 backend 最近 20 筆 `{at, from, to, reason}`。

啟動時 `init_logging("info")` 在讀設定檔之前就執行（要先能記 plugin 啟動與設定錯誤），讀完設定後才用 reload handle 換 level。`[log]` 目前只有 `level`，reload 可以換。其他只能重啟的設定（`store`、`dns.listen`、`metrics.listen`、`metrics.allow`、`peers.listen`）在 reload 時是驗證錯誤，整個 reload 被拒絕（`cli.rs`，訊息格式「<key> settings require restart」）。

## Goals / Non-Goals

**Goals:**

- 一種機器可讀的 log 輸出，不用寫文字解析。
- 一份明確的穩定欄位承諾，與現有的稽核行承諾同一套規則。
- 手冊說明怎麼用 `emfor status --json` 或 JSON log 接 SNMP trap 與任意指令。

**Non-Goals:**

- 內建 SNMP、內建狀態變化 hook、log 輪替、其他 log 行加 `event`、改現有欄位名。
- 讀設定檔之前的 log 行也用 JSON。

## Decisions

### `format` 是 `[log]` 的鍵，改它要重啟，reload 拒絕

`[log] format = "text"`（預設）或 `"json"`。reload 把 `format` 改掉時，照其他只能重啟的設定的慣例，加一個驗證錯誤（`log.format`，訊息「log format settings require restart」），整個 reload 被拒絕、沿用舊設定。換 `level` 仍可 reload。理由：同一條慣例，維運不必學新規則；而且在跑的 pipeline 中途換格式本來就是壞事。

### 讀設定檔之前的行維持文字

`init_logging("info")` 之後、設定讀完之前，格式還不知道（plugin 啟動、`config file read`、設定錯誤）。做法：fmt layer 也包進 reload layer，讀完設定後與 level 一起換；這之前的行（啟動期的少數幾行）是文字。手冊要寫明，並建議 JSON 的接收端把非 JSON 行丟到另一個通道。替代做法是先從主檔預讀 `[log] format`，但那要重複分層設定的解析，不值得。`RUST_LOG` 只覆蓋 level，不影響 `format`。

### JSON 行的形狀：欄位攤平在最上層

用 `tracing_subscriber::fmt::layer().json().flatten_event(true)`，關掉 span 與 current span，不用 ANSI。一行一個物件：`timestamp`、`level`、`target`、`message`，加上事件自己的欄位（`service`、`backend`、`from`、`to`、`reason`、`change`、`field` 等）。理由：攤平後 `jq .service` 就取得到，NMS 的欄位對應最簡單；預設的巢狀 `fields` 多一層，沒有好處。風險：事件欄位若與 `timestamp`、`level`、`target` 同名，會產生重複的 key；實作時要確認現有的 tracing 呼叫沒有這種欄位名，並在承諾裡禁止之後新增。

### 新增 `event` 識別欄位，只加在狀態變化行

狀態變化行加 `event = "backend_state_changed"`。`event` 不叫 `backend_changed`，因為稽核行的 `change=backend_changed` 是設定變更（weight、drain 等），兩個名字相同會讓接收端混淆。稽核行本來就有穩定的 `change`，不再加 `event`。代價：文字模式的狀態變化行會多一個 ` event=backend_state_changed`（欄位附在行尾）；措辭本來就不保證，所以不算破壞，但這是文字輸出唯一的變動。

### 穩定欄位承諾

穩定：狀態變化行的 `event`、`service`、`backend`、`from`、`to`、`reason`；稽核行現有的 `change` 與各欄位（第 12 章已列）；JSON 的 `timestamp`（RFC 3339）與 `level`。不穩定：`message` 的文字、`target`、其他行的欄位、文字模式的措辭。新增欄位不算破壞；改名或移除要走 1.0 前小版本可以不相容的現有規則（`docs/compatibility.md`）。`docs/compatibility.md` 與第 15 章的「Log lines：不穩定」改成「除下列欄位外不穩定」。

### SNMP 與 hook 不內建，手冊示範

SNMP trap 要向 IANA 申請 enterprise OID、要維護 MIB，SNMPv3 要 MD5/SHA/AES，與第 13 章 FIPS 一節的說法衝突。內建 hook 要處理 timeout、不阻塞檢查迴圈、震盪時合併或丟棄、執行身分與 FreeBSD 行為，工作量中等，而現成的 `emfor status --json` 的 `history` 已經夠輪詢。手冊示範兩種：一支輪詢 `emfor status --json` 比較 `history` 後呼叫 `snmptrap` 的腳本，以及 net-snmp 的 `extend`（讓管理站用 SNMP 取 `emfor status --json` 的輸出）。手冊要寫明哪些腳本被實際執行過、哪些沒有。

## Implementation Contract

**Observable behavior**

- `[log] format` 沒設或為 `"text"`：輸出與現在相同，只有狀態變化行行尾多 ` event=backend_state_changed`。
- `format = "json"`：讀完設定之後的每一行 log 是一個 JSON 物件，含 `timestamp`、`level`、`target`、`message` 與事件欄位，攤平在最上層。狀態變化行有 `event`、`service`、`backend`、`from`、`to`、`reason`；稽核行有 `change` 與它的欄位。讀完設定之前的行仍是文字。
- `format` 不是字串、或不是 `"text"` / `"json"`，為 `log.format` 錯誤（訊息「must be one of text, json」）。
- reload 把 `format` 改成與在跑的不同：reload 被拒絕（`log.format` 錯誤，「log format settings require restart」），沿用舊設定；`level` 改變仍可 reload。`RUST_LOG` 設了也不影響 `format`。
- `emfor status --json` 不變。

**Interface / data shape**

- `config.rs`：`LogConfig.format: LogFormat`（`Text` 預設、`Json`），`log()` 的允許鍵加 `format`。
- `cli.rs`：fmt layer 包進可替換的 layer；啟動後與 level 一起套用 `format`；reload 驗證加 `log.format` 重啟檢查。
- `schedule.rs`：狀態變化行加 `event`。
- `Cargo.toml`：`tracing-subscriber` 加 `json` feature；`Cargo.lock` 與 `fuzz/Cargo.lock` 一併更新。

**Failure modes**

- 換 layer 失敗（reload handle 的錯誤）：記 `error`「cannot set log format: …」，沿用文字。
- `format = "json"` 的 JSON 寫入錯誤（標準錯誤關閉）：與文字模式相同，不另外處理。

**Acceptance criteria**

- `config.rs` 單元測試：`format` 範例表每一列（沒設、`"text"`、`"json"`、`"yaml"`、`true`）。
- 整合測試（`tests/` 下，起 daemon 子行程）：`format = "json"` 時，`emfor started` 之後的每一行 stderr 都能被 JSON 解析，狀態變化行有 `event`、`service`、`backend`、`from`、`to`、`reason`，稽核行有 `change`；`format` 預設時行不是 JSON；reload 改 `format` 被拒絕且 daemon 繼續用舊格式。
- `schedule.rs` 單元測試：狀態變化行帶 `event`（以 tracing_test 檢查）。
- 現有的 tracing 呼叫沒有名為 `timestamp`、`level`、`target` 的欄位（grep 確認，寫進任務的驗證）。
- 手冊的腳本：用假的 `emfor status --json` 輸入與取代 `snmptrap` 的指令乾跑，輸出正確的 trap 參數；沒實際對 SNMP 管理站測試的部分在手冊明說。
- `cargo check --manifest-path fuzz/Cargo.toml --bins --locked` 通過；`tools/docker-test.sh` 通過；`python3 tools/check-manual-links.py` 回報 0 problems。

**Scope boundaries**

- In scope：`[log] format`、JSON 輸出、`event`、穩定欄位承諾、手冊與 compatibility 文件、CHANGELOG、兩個 spec。
- Out of scope：SNMP、hook、log 輪替、其他 `event`、`status --json` 的變更。

## Risks / Trade-offs

- **啟動期的文字行：** JSON 接收端會在最前面看到幾行非 JSON。手冊寫明；不另外處理。
- **欄位名撞鍵：** `flatten_event` 下事件欄位與 `timestamp`／`level`／`target` 同名會產生重複 key。現在沒有（要驗證），之後新增的人要知道；承諾裡寫明禁止。
- **文字輸出多一欄：** 行尾多 ` event=…`，依賴文字措辭的人會看到差異；措辭本來就不保證。
- **新依賴：** `tracing-serde` 是新 crate（`tracing-subscriber` 的 `json` feature 帶入），要過 `cargo deny`。

## Migration Plan

1. 升級後行為不變（預設 `text`）。
2. 要用 JSON 的人在設定加 `[log] format = "json"` 並重啟（reload 會被拒絕，訊息說明要重啟）。
3. 退回：拿掉 `format` 並重啟。
