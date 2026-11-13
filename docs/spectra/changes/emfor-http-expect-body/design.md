## Context

`http` 檢查以 reqwest 送一個 `GET`，不跟隨 redirect，狀態碼落在 `expect_status` 內就是 `up`，回應內容從來不讀。`tcp` 檢查已經有 `expect`：在最多 16384 位元組內找一段文字，找不到時原因是 `expect: got "<前 64 位元組>"`。整個檢查受服務的 `timeout` 限制，逾時為 `down`（`timeout`）。

## Goals / Non-Goals

**Goals:**

- 用一段文字判斷健康端點的內容，寫法和 `tcp` 的 `expect` 一致；
- 讀內容的量有上限，不會因為大回應拖慢或吃記憶體。

**Non-Goals:**

- regex、JSON path、多段比對、「不可包含」的比對；
- 其他 HTTP 方法（HEAD、POST）、自訂 request header、跟隨 redirect；
- 解壓縮內容（daemon 不送 `Accept-Encoding`，伺服器應回未壓縮的內容）。

## Decisions

### expect_body 沿用 tcp expect 的比對方式

逐位元組、區分大小寫的子字串比對；`expect_body` 必須是 1 到 16384 位元組的字串。原因與 `tcp` 相同：`expect: got "..."` 的寫法維運人員已經熟悉，文件也可以互相參照。

替代方案：regex。否決，因為多一個相依與一種語法，而健康端點的內容幾乎都能用一段固定文字判斷。

### 內容最多讀 65536 位元組，找到即停止

內容一段一段讀進來，累積在緩衝區裡比對，這樣跨段的文字也能找到。找到就停止讀取並關閉連線。讀滿 65536 位元組或內容結束仍沒找到時為 `down`。

上限比 `tcp` 的 16384 大，是因為 Spring Boot Actuator 這類健康 JSON 常超過 16 KB。讀內容的時間算在同一個 `timeout` 裡。

### 狀態碼先判斷

狀態碼不符時維持原本的原因（`unexpected status N`），也不讀內容。這樣原因指向最先出錯的地方，也省下讀內容的時間。

## Implementation Contract

**行為**

- 設了 `expect_body`：狀態碼符合 `expect_status`，而且內容前 65536 位元組內含 `expect_body`，才是 `up`；
- 沒設 `expect_body`：行為與現在相同，不讀內容。

**選項**

- `expect_body`：字串，1 到 16384 位元組。空字串為 `must not be empty`，過長為 `must be at most 16384 bytes`，非字串為 `must be a string`。

**原因**

- `expect_body: got "<內容前 64 位元組，escape_ascii>"`（`down`）；
- `body: <錯誤鏈>`（`down`，讀內容失敗，例如連線中斷）；本機 I/O 錯誤的分類與送出請求時相同。

**驗收**

- http.rs 單元測試涵蓋：spec 範例表 expect_body 每一列、跨兩段送出的內容、超過 65536 位元組的內容、選項驗證錯誤；
- 拿掉 `expect_body` 的比對時，新增的測試會失敗；
- macOS 與 Debian 映像的 fmt、clippy、test 全部通過。

**範圍**

- 範圍內：http 檢查、選項驗證、文件、範例、CHANGELOG；
- 範圍外：Non-Goals 所列各項，以及 `tcp` 檢查。

## Risks / Trade-offs

- [設了 `expect_body` 的服務，延遲包含讀內容的時間，`fastest` 的排序會跟著變] → 同一服務的所有 backend 都用同一個檢查，相對比較不受影響；文件註明。
- [健康端點回壓縮內容] → daemon 不送 `Accept-Encoding`，正常伺服器會回未壓縮的內容；若仍收到壓縮內容，原因的前 64 位元組會看得出來。
