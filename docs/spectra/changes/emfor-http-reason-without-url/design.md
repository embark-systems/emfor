## Context

`error_chain` in `crates/emfor-daemon/src/checker/http.rs` 把 `reqwest::Error` 與其 `source()` 鏈用 `: ` 串成原因。reqwest 在最上層錯誤的 `Display` 末端附上 ` for url (<URL>)`，URL 含 `path` 與 query。

## Goals / Non-Goals

**Goals:**

- http 檢查的原因不含請求 URL。

**Non-Goals:**

- `emfor-check` 的其他協定與 http3；`path` 驗證規則。

## Decisions

### 在轉成原因前呼叫 `without_url`

`reqwest::Error::without_url(self) -> Self` 去掉內嵌的 URL。呼叫點是兩個 `match` 的 `Err(e)` 分支（請求失敗、讀 body 失敗）：`has_local_io_error(&e)` 先以參考判斷，之後把 `e` 交給一個小函式 `client_reason(e: reqwest::Error) -> String`，內容為 `error_chain(&e.without_url())`。不在 `error_chain` 內做，因為它收的是 `dyn Error`，其他呼叫者（若有）不一定是 reqwest。

## Implementation Contract

- **行為**：http 檢查失敗的 `down` 或 `error` 原因不含請求 URL（含 path 與 query）；失敗種類仍可從原因看出（連線被拒、逾時、TLS 錯誤等）。
- **介面**：私有函式 `client_reason`；沒有設定、指標或訊息格式的改變。
- **失敗模式**：`without_url` 不會失敗；source 鏈裡的訊息照舊串接。
- **驗收**：新測試對 `path = "/health?token=s3cret"` 觸發三種失敗，各斷言原因不含 `s3cret`、`token=`、`/health`，且仍含失敗種類關鍵字：連線被拒（對一個沒有人聽的 port）、讀 body 失敗（伺服器送出 Content-Length 後中斷）、TLS 失敗（對只講明文的伺服器用 `https`）。既有 http 測試與 `tools/docker-test.sh` 全部通過。
- **範圍內**：`checker/http.rs`、CHANGELOG。**範圍外**：見 Non-Goals。

## Risks / Trade-offs

- 去掉 URL 之後，原因少了「連到哪個路徑」的資訊；服務設定本來就有 `path`，管理者查得到。
- 反向風險：reqwest 的某些版本可能在 source 鏈的較深層也放 URL；測試以真實的三種失敗斷言，若有就會失敗。

## Open Questions

None
