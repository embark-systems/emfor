## Problem

在沒有系統 CA 憑證的主機上（最小化安裝的 Debian 或 RHEL，沒裝 `ca-certificates`），每個 `http` 檢查都以 `error` 結束，原因是 `cannot build http client: builder error`，連 `scheme = "http"`（完全不做 TLS）也一樣，要裝了 `ca-certificates` 才能用。deb 與 rpm 套件已經依賴 `ca-certificates`（TODO 60 的 A），所以用套件安裝不受影響；手動安裝、容器映像或從原始碼建置的環境會踩到。

## Root Cause

`HttpChecker::client_for` 對兩種 scheme 用同一個 builder 建 `reqwest::Client`，只傳 `tls_danger_accept_invalid_certs(!tls_verify)`。reqwest 0.13.5 搭配 `rustls`，在憑證驗證開著（預設）且沒有額外根憑證時，建 client 的過程就會建立 `rustls_platform_verifier::Verifier`，它會讀系統根憑證，讀不到就讓建置失敗（依 reqwest 0.13.5 原始碼 `async_impl/client.rs` 判斷）。這個建置與 scheme 無關，所以不會開 TLS 握手的 `http` 也需要用不到的 CA 憑證。驗證關掉時 reqwest 改裝一個接受所有憑證的 verifier、不讀根憑證，所以 `tls_verify = false` 現在就不會失敗。後面這一點是讀原始碼的推論，第一個任務會在容器內重現並確認。

## Proposed Solution

`scheme = "http"` 時用「驗證關閉」建 client（`tls_danger_accept_invalid_certs(!tls_verify || !https)`）：client 不再讀系統根憑證，而這個 scheme 本來就不會有 TLS 握手，所以沒有削弱任何東西。這移除根因（為沒有 TLS 的 scheme 建立驗證狀態）。`https` 在驗證開著又沒有系統 CA 憑證時仍然建不起來，這是對的，但原因文字會說明缺什麼、該怎麼做：點出系統 CA 憑證與 `tls_verify = false`，不再只有 `builder error`。

## Non-Goals

- 不改 `https` 的驗證：仍預設開啟，除非 `tls_verify = false`，系統根憑證照樣讀。
- 不替 `http` 檢查新增 `ca_file`（它現在沒有這個選項）。
- 不改 tcp 檢查的訊息（它已經寫 `no system CA certificates found; set ca_file`），也不改套件對 `ca-certificates` 的依賴。
- 不碰 plugin（`emfor-check`）。

## Success Criteria

- 在沒有 `ca-certificates` 的 Debian 容器內，`scheme = "http"` 的檢查對本機伺服器回 200 時為 `up`，回 503 時為 `down`。
- 同一個容器內，驗證開著的 `https` 檢查為 `error`，原因點出系統 CA 憑證與 `tls_verify = false`。
- 裝了 CA 憑證時，既有的 HTTP 測試與完整 gate 不改而通過。

## Impact

- Affected specs: `health-checking`（需求「HTTP checker」）
- Affected code:
  - Modified: `crates/emfor-daemon/src/checker/http.rs` 與其測試、`docs/manual/04-health-checks.md`（與 `docs/manual.zh_TW/04-health-checks.md`）、`CHANGELOG.md`
  - New: 無
  - Removed: 無
