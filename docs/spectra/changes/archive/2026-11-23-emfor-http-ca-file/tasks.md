## 1. 選項與驗證

- [x] 1.1 實作 requirement「HTTP checker」的 `ca_file` 驗證（design：「ca_file 取代系統 CA，不是附加」、「憑證從 emfor-tls 取，不另寫 PEM 解析」、「scheme = "http" 設 ca_file 是驗證錯誤」、「ca_file 與 tls_verify = false 同時設不算錯」）。`crates/emfor-daemon/src/checker/http.rs` 的 `ALLOWED` 加 `ca_file`，`HttpOptions` 加 `ca_file: Option<PathBuf>`，`parse_options` 以 `emfor_tls::check_ca_file` 驗證；`scheme = "http"` 設 `ca_file` 回 `requires scheme = "https"`；非字串回 `must be a string`；與 `tls_verify = false` 同設通過。驗證：`http.rs` 單元測試涵蓋 spec 範例表「ca_file options」的每一列，測試帶 `// spectra-scenario: health-checking / ca_file validation` 與 `... / ca_file validation / ca_file options` 註解；`tools/docker-test.sh 'cargo test -p emfor-daemon --lib checker::http'` 通過，且新測試先在只改測試時失敗（`unknown key`）。

## 2. 信任與換檔

- [x] 2.1 [after: 1.1] 讓 `ca_file` 真的被信任（design：「ca_file 取代系統 CA，不是附加」）：`HttpChecker::client_for` 在 `ca_file` 為 `Some` 且驗證開啟時，以 `emfor_tls::trusted_certificates` 取得憑證、轉成 `reqwest::Certificate` 並用 `tls_certs_only` 建 client，不讀系統 CA；建 client 的失敗提示在設了 `ca_file` 時改指 `ca_file`；檔案失效時結果為 `error` 且原因含 `ca_file`（design：「檢查時 CA 檔失效是 error，不是 down」）。驗證：`http.rs` 單元測試（用 `emfor-tls` 的 `testing` feature 產生測試 CA 與伺服器憑證，本機起 HTTPS 伺服器；「沒有系統 CA 時仍 `up`」另在 `crates/emfor-daemon/tests/http_without_ca.rs`）涵蓋情境「Private CA through ca_file」（含 CA 的檔案得 `up`）與「ca_file replaces the system CA certificates」（無關 CA 的檔案不得 `up`），並在檔案被刪除後得 `error`；註解帶對應的 `spectra-scenario`；`tools/docker-test.sh` 內執行通過。
- [x] 2.2 [after: 2.1] 實作 design「換掉 ca_file 不必 reload」：`emfor-tls` 新增公開函式回報 `ca_file` 的變更標記（沿用 `FileStamp` 的欄位），`http.rs` 每次檢查先取標記，與快取 client 建立時的標記不同就清掉快取、重建；沒變就沿用。驗證：同一組單元測試再加情境「Replaced ca_file takes effect without a reload」：先以含 CA A 的檔案檢查得 `up`，把檔案換成只含無關 CA B 的檔案，下一次檢查不得 `up`，全程不 reload；`emfor-tls` 單元測試驗證標記在檔案內容、大小或 mtime 改變時改變、不變時相同。`tools/docker-test.sh` 內通過。

## 3. 文件

- [x] 3.1 [after: 2.2] `docs/manual/06-health-checks.md` 與 `docs/manual.zh_TW/06-health-checks.md` 的 `http` 選項表加 `ca_file`（PEM 檔，只信它不信系統 CA，絕對路徑，換檔不必 reload），並說明私有 CA 的作法；`docs/manual/16-troubleshooting.md` 與中文版中「install ca-certificates, or set tls_verify = false」的建議補上 `ca_file`；`CHANGELOG.md` 的 Unreleased 加一條。驗證：內容審閱，中英與 spec 用語一致；再跑 `tools/docker-test.sh` 的完整檢查（fmt、clippy、`cargo test --workspace --all-features`）通過。
