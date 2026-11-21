## Problem

`http` 檢查失敗時，原因（reason）會帶完整的請求 URL，包括 `path` 的 query。`path = "/health?token=abc"` 是常見的用法（`path` 的驗證明確允許 query），後端拒絕連線或 TLS 失敗時，原因會變成 `error sending request for url (http://ip:port/health?token=abc): ...`。原因會被寫進 store、日誌和 `emfor status`，所以 token 洩漏到任何能讀 store 的本機使用者（store 預設對 daemon 群組可讀）。這是 Fable 審查（2026-11-21）的項目；URL 由 reqwest 附加是程式碼與其錯誤格式的推論，下面的成功條件包含用真實失敗驗證。

## Root Cause

`crates/emfor-daemon/src/checker/http.rs` 的 `error_chain` 直接用 `reqwest::Error` 的 `Display` 組出原因。reqwest 的錯誤訊息預設附上 ` for url (<URL>)`；它提供 `Error::without_url` 去掉 URL，但沒有人呼叫（程式裡 `without_url` 出現 0 次）。

## Proposed Solution

在兩處把 `reqwest::Error` 轉成原因之前先呼叫 `without_url()`（請求本身的失敗，以及讀 body 的失敗），其餘不變。URL 的 host 與 port 就是後端位址，本來就是服務設定的一部分，所以只去掉 URL 沒有損失資訊。

## Non-Goals

- 不改 `emfor-check` 的 `http3` 檢查與其他協定（它們沒有被審查者指出這個問題，也沒有在這裡驗證；實作時順手 grep，若有同樣模式再另開變更）。
- 不改 `path` 的驗證規則，也不禁止 query。

## Success Criteria

- 對 `path = "/health?token=s3cret"` 的檢查，連線被拒絕、TLS 失敗、讀 body 失敗三種情況的原因都不含 `s3cret`、`token=` 或 path，並仍然說明失敗的種類（例如 `connection refused`）。
- 既有 http 測試全部照舊通過；`tools/docker-test.sh` 全部通過。

## Impact

- Affected specs: `health-checking`（requirement "HTTP checker"）
- Affected code:
  - Modified: `crates/emfor-daemon/src/checker/http.rs`、`CHANGELOG.md`
  - New: (none)
  - Removed: (none)
