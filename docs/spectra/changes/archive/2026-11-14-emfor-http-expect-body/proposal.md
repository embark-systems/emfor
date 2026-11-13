## Why

`http` 檢查只看狀態碼，但很多健康端點不論好壞都回 200，把真正的狀態寫在內容裡（例如 `{"status":"DOWN"}` 或 Elasticsearch `_cluster/health` 的 `"status":"red"`）。這類 backend 現在一律是 `up`。加上內容比對後，不必寫 plugin 或 exec script 就能正確判斷。

## What Changes

- `http` 檢查新增選項 `expect_body`：回應內容必須包含這段文字（逐位元組比對、區分大小寫），和 `tcp` 的 `expect` 一樣。
- 先比對狀態碼；狀態碼符合後才讀內容，最多讀 65536 位元組，找到就停止讀取。
- 沒找到為 `down`，原因 `expect_body: got "<內容前 64 位元組，escape 後>"`；讀內容時出錯為 `down`，原因以 `body: ` 開頭。
- 沒設 `expect_body` 時行為不變，也不讀內容。
- 文件（手冊第 4 章中英文）、examples/emfor.toml、CHANGELOG 一併更新。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `health-checking`：HTTP checker 新增 `expect_body`。

## Impact

- 程式：crates/emfor-daemon/src/checker/http.rs。
- 測試：同一檔案的單元測試（本機假 HTTP 伺服器）。
- 文件：docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、examples/emfor.toml、CHANGELOG.md。
- 不新增相依套件（reqwest 已在使用）；store 格式、外掛協定不變。設了 `expect_body` 的服務，量到的延遲會包含讀內容的時間。
