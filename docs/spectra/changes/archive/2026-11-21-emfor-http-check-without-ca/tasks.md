## 1. 重現與修正

- [x] 1.1 重現失敗並確認 design 的前提：在沒有 `ca-certificates` 的 `debian:bookworm-slim` 容器內，用目前原始碼的 release 版 `emfor` 跑 `scheme = "http"` 的 `http` 檢查（對本機伺服器），記下原因文字（預期 `cannot build http client: builder error`），再用 `tls_verify = false` 確認不會失敗；把實際看到的原因文字寫進 design.md（Context 的「尚未驗證」段）。驗證：容器輸出已寫進 design.md；若 `tls_verify = false` 沒有避開失敗，停下來先修正 design 再做 1.2。
- [x] 1.2 [after: 1.1] 在 `crates/emfor-daemon/src/checker/http.rs` 實作「純 `http` scheme 關閉驗證」與「`https` 的原因文字說明缺什麼」：builder 參數，以及把建置錯誤轉成原因文字的函式（只對 `tls_verify = true` 的 `https` 加提示）。驗證：裝了 CA 憑證時，`tools/docker-test.sh` 內既有的 `http` 單元測試不改而通過。
- [x] 1.3 [after: 1.2] 加單元測試（https 驗證開著會帶提示；`http` 與 `tls_verify = false` 不帶），並在沒有 `ca-certificates` 的 `debian:bookworm-slim` 容器內確認：`http` 檢查 200 為 `up`、503 為 `down`，驗證開著的 `https` 檢查給出新的原因。破壞檢查：把 builder 參數改回 `!tls_verify`，容器內 `http` 的檢查必須失敗。驗證：單元測試在 Docker 內通過；容器輸出與破壞檢查的失敗記進 commit message。

## 2. 文件與 gate

- [x] 2.1 [after: 1.3] 更新 `docs/manual/04-health-checks.md` 與 `docs/manual.zh_TW/04-health-checks.md`（`http` 一節：`scheme = "http"` 不需要 CA 憑證，`https` 除非 `tls_verify = false` 否則需要），加 `CHANGELOG.md` Unreleased 條目，並讓「HTTP checker」需求的 delta 與最終的原因文字一致。驗證：`python3 tools/check-manual-links.py`、完整 `tools/docker-test.sh` gate、`spectra analyze` 與 `spectra validate` 沒有 Critical。
