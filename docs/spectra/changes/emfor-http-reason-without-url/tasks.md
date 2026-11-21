## 1. 修正與測試

- [x] 1.1 `crates/emfor-daemon/src/checker/http.rs` 新增私有函式 `client_reason(e: reqwest::Error) -> String`（先 `without_url()` 再 `error_chain`），請求失敗與讀 body 失敗兩處的 `error_chain(&e)` 改用它（設計決定「在轉成原因前呼叫 `without_url`」，實作 spec health-checking 的 "HTTP checker" 的新句子）。完成時：`checker/http.rs` 中除 `client_reason` 外沒有把 `reqwest::Error` 直接交給 `error_chain`。
- [x] 1.2 [after: 1.1] 在 `checker/http.rs` 的測試模組新增測試，對 `path = "/health?token=s3cret"` 觸發連線被拒、讀 body 失敗、TLS 失敗（對只講明文的伺服器用 `https`），各斷言原因不含 `s3cret`、`token=`、`/health`，且含失敗種類的關鍵字（情境 "Reason without the request URL"）。完成時：測試在 Debian 容器通過；暫時把 1.1 還原時（或 stash 該修改）至少連線被拒那一個測試會失敗，證明測試有效（結果寫進 CHANGELOG）。

## 2. 文件與檢查

- [x] 2.1 [after: 1.2] `CHANGELOG.md` 的 Unreleased 加 bug fix 條目（含 Checked 與 Not verified：`emfor-check` 其他協定與 http3 沒有檢查；真實 Redis 無關）。在 Debian 容器跑 fmt、clippy `-D warnings` 與全部測試，把結果數字寫進條目。
