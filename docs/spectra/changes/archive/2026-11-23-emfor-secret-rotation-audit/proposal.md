## Why

聯防密鑰內容輪換（同一個路徑的 `secret_file` 換成新內容，手冊第 11 章建議用 rename-over）是維運最常做、也最需要事後追查的一件事。reload 時 `secret_file` 與 `previous_secret_file` 的「路徑」變更已有結構化稽核行（`change=peers_changed`），但「內容」變更不經 reload，由執行中的監看讀進來，目前唯一的紀錄是一行沒有 `change` 欄位、也沒有檔案路徑的 `info`：`peer secret changed; messages are signed with the previous one for 60s more and it is accepted for 120s`。事後要查「這台什麼時候換了密鑰」只能靠這行文字，格式與其他稽核行不一致，一台有多個設定來源或多次輪換時也看不出是哪個檔案。這是 docs/TODO.md 項目 73 第 4 點（稽核）的缺口。

## What Changes

- 同一路徑的 `secret_file` 內容輪換成功時，原本那行 `info` 補上兩個結構化欄位：`change=peers_secret_rotated` 與 `secret_file=<路徑>`；訊息文字保留。只有路徑，不含密鑰內容、雜湊或指紋（手冊第 12 章「What a reload changed」已承諾 log 不含密鑰內容或其雜湊，維持這個承諾）。
- reload 讓 `secret_file` 路徑改變、而新路徑的內容與舊密鑰不同時，同一個輪換也會記這行，`secret_file` 是新路徑；內容相同時不記，與現在一致。
- 新增的是 `change` 值與欄位，沒有改既有欄位或 `change` 值，符合手冊第 15 章「稽核欄位名稱與 `change` 值跨版本穩定」。
- 補測試：`secret_file` 路徑在 reload 改變會產生 `peers_changed`（`field=secret_file`、`applies=now`）的稽核行；內容輪換行有 `change` 與路徑、不含內容；內容相同不記。
- 手冊第 11 章（輪換密鑰）與第 12 章（reload 稽核）中英各加一段說明：內容輪換不經 reload，稽核行是 `change=peers_secret_rotated`。CHANGELOG 加一條。

## Non-Goals

- 不記密鑰內容、雜湊或指紋。
- 不改輪換行為與時間窗（簽 60 秒、收 120 秒）、不改 `previous_secret_file` 只在啟動時讀的行為。
- 不改 `audit.rs` 的每次 reload 最多 50 行的上限；這一行不是 reload 稽核行，不計入。
- 不改啟動時那幾行 `info`（`peer secret changed Ns ago`、`previous_secret_file is ignored`）。
- drain 已有 `change=backend_changed`（`field=drain`）與測試，不改。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: 新增一個 requirement，規定內容輪換成功時記一行帶 `change=peers_secret_rotated` 與 `secret_file` 的 `info`，且不含密鑰內容或其雜湊。

## Impact

- `crates/emfor-daemon/src/peers/task.rs`：`apply_secret` 取得目前讀的路徑，輪換成功時帶結構化欄位記 `info`；呼叫端（`SecretReader::refresh` 的兩處）與該檔既有的測試呼叫要跟著調整簽章。
- `crates/emfor-daemon/src/audit/tests.rs`、`peers/task.rs` 的測試：新增上述測試。
- `docs/spectra/specs/peer-group/spec.md`（delta）；`docs/manual/11-peer-groups.md`、`docs/manual/12-operating.md` 與 `docs/manual.zh_TW/` 對應章節；`CHANGELOG.md`。
