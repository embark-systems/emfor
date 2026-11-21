## Context

`HttpChecker::client_for`（`crates/emfor-daemon/src/checker/http.rs`）在阻塞執行緒上為每個 checker（有網域 `host_header` 時每個 backend 一個）建一個 `reqwest::Client`，設定 `redirect(Policy::none())`、`no_proxy()`、`pool_max_idle_per_host(0)` 與 `tls_danger_accept_invalid_certs(!self.opts.tls_verify)`；`scheme = "http"` 與 `"https"` 用同一個 builder。建置錯誤變成 `CheckResult::Error("cannot build http client: <e>")`。reqwest 是 0.13.5，`default-features = false, features = ["rustls"]`（`crates/emfor-daemon/Cargo.toml`）。

已讀 reqwest 0.13.5 原始碼（`src/async_impl/client.rs` 的 rustls 分支）：驗證關閉時 client 用 `NoVerifier`、不讀根憑證；驗證開著且沒有額外根憑證時呼叫 `rustls_platform_verifier::Verifier::new`，它讀系統根憑證，失敗就回傳 builder 錯誤。TODO 60 先前的調查在沒有 `ca-certificates` 的系統上看到 `cannot build http client: builder error`，`scheme = "http"` 也一樣，裝了套件後正常。tcp 檢查加 `tls = true` 已經會說 `no system CA certificates found; set ca_file`；deb 與 rpm 套件依賴 `ca-certificates`。

**任務 1.1 的重現結果（`emfor-clients-test` 容器，Debian 13，刪掉 `/etc/ssl/certs` 與 `/usr/share/ca-certificates`，未設 `SSL_CERT_*`，目前原始碼的 release 版 `emfor`，後端是本機的 Python HTTP 伺服器）：** `scheme = "http"` 的檢查（`/` 與 `/bad`）與 `scheme = "https"` 驗證開著的檢查都得到 `unknown | cannot build http client: builder error`，重現了 TODO 60 的現象，所以 `http` 沒有 TLS 也建不起 client；`scheme = "https"` 加 `tls_verify = false` 的 client 建得起來（之後的失敗 `received corrupt message of type InvalidContentType` 是因為後端是明文伺服器，與憑證無關）。這支持「驗證關閉就不讀根憑證」的推論。重現只做了一次，只在這個容器的 Debian 13 上；`rustls_platform_verifier` 沒有根憑證時的原始錯誤文字被 reqwest 吞成 `builder error`，所以看不到。

## Goals / Non-Goals

**Goals:**

- `scheme = "http"` 在沒有系統 CA 憑證的主機上可用。
- `https` 在驗證開著且沒有 CA 憑證時，以說明缺什麼的原因失敗。

**Non-Goals:**

- 不改 `https` 怎麼驗證、不替 `http` 檢查加 `ca_file`、不改 tcp 檢查、plugin 與套件依賴。

## Decisions

### 純 `http` scheme 關閉驗證

`client_for` 改傳 `tls_danger_accept_invalid_certs(!self.opts.tls_verify || !self.opts.https)`。`scheme = "http"` 的 URL 是 `http`，又不跟隨轉址，所以不會有 TLS 握手，verifier 要檢查的東西根本用不到；建它只會去讀可能不存在的根憑證。`https` 仍然是 `!tls_verify`。捨棄的做法：為 `http` 建一個沒有 TLS 支援的 client（要第二組 reqwest feature 或第二種 client 型別，程式更多、效果相同）；只靠套件依賴 `ca-certificates`（已做，但救不了手動安裝與映像）。

### `https` 的原因文字說明缺什麼

`https`、`tls_verify` 為真、建置失敗時，原因是 `cannot build http client: <e>; https checks need the system CA certificates unless tls_verify = false`。只在這種情況加提示，其他建置錯誤文字不變。`<e>` 本來就不含 request URL（builder 錯誤沒有），所以「HTTP client 產生的原因不含 URL」這條規則不受影響。

## Implementation Contract

**行為**

- `scheme = "http"`：沒有系統 CA 憑證也能建 client，結果與以前相同。
- `scheme = "https"`、`tls_verify = true`、沒有系統 CA 憑證：`error`，原因以 `cannot build http client:` 開頭，含 `system CA certificates` 與 `tls_verify = false`。
- 裝了 CA 憑證時，兩種 scheme 都沒有任何變化。

**介面**

- 只動 `crates/emfor-daemon/src/checker/http.rs`（builder 參數與錯誤文字）；加一個把建置錯誤轉成原因文字的小函式，讓單元測試不需要缺 CA 憑證的主機就能測到。沒有選項、設定鍵、通訊格式或 store 的變動。

**驗收**

- 單元測試：https 且驗證開著的建置錯誤會帶提示；`http` 與 `tls_verify = false` 不帶提示。
- 在沒有 `ca-certificates` 的 `debian:bookworm-slim` 容器內：`http` 檢查對本機伺服器 200 為 `up`、503 為 `down`；驗證開著的 `https` 檢查給出新的原因。修改前在同一個容器內重現 `builder error`。
- 完整 Docker gate 通過。

**範圍**

- 範圍內：builder 參數、原因文字、測試、手冊 04 章（中英）、CHANGELOG、「HTTP checker」需求的 delta。
- 範圍外：見 Non-Goals。

## Risks / Trade-offs

- `https` 的驗證維持原樣，只有 `http` scheme 不建 verifier。若 reqwest 之後在驗證關閉時仍讀根憑證，任務 1.1 的容器測試與驗收會看到。
- 提示文字會附在「驗證開著的 `https` 檢查」的任何建置錯誤上；若有別的原因（例如無效的 TLS 版本設定，這段程式不會設定）會帶來誤導。接受：目前這個建置沒有其他會失敗的輸入。

## Migration Plan

沒有資料、設定或介面變動。回退：換回上一版 binary。

## Open Questions

None
