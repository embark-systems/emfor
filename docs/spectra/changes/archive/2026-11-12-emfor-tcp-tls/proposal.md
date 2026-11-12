## Why

內建 `tcp` 檢查無法檢查 TLS 服務：對 LDAPS、IMAPS、走 TLS 的 redis 這類 port，只能確認 port 有開，看不出 TLS 握手是否成功、憑證是否過期或名稱不符。HAProxy 以 `check-ssl` 涵蓋這種情況。emfor-check 已經有一套經過實機測試的 TLS 選項與實作，tcp 檢查沿用同一套，使用者只需要學一種寫法。

## What Changes

- 內建 `tcp` 檢查新增 `tls`（預設 `false`）、`tls_verify`（預設 `true`）、`server_name`（預設後端 IP）、`ca_file`（預設系統 CA）四個選項，意思與 emfor-check 完全相同；`tls = false` 時設定後三者為設定錯誤。
- `tls = true` 時連上後先做 TLS 握手，不退回明文：握手或憑證驗證失敗為 down，原因以 `tls: ` 開頭；明文 port 不回應握手時在剩餘時間的 90% 內回 `tls: no answer to the TLS handshake (does the port speak TLS?)`。握手成功後，有 `expect` 就在加密連線上做 `send` / `expect`，沒有則為 up 並正常關閉 TLS 連線。檢查時讀不到 `ca_file` 為 error。
- 新增內部 crate `crates/emfor-tls`：把 emfor-check 的 TLS 程式碼（信任清單、`tls_verify = false` 的驗證器、握手時間預算、`server_name` 與 `ca_file` 的檢查）移過去，由 daemon 的 tcp 檢查與 emfor-check 共用；emfor-check 的行為不變。
- 文件：`examples/emfor.toml` 加上 tcp TLS 範例；`docs/spectra/config.yaml` 記錄 tcp 的 TLS 選項與 emfor-tls。

**前提**：本 change 修改的「TCP checker」需求以 `emfor-tcp-expect` 完成後的版本為基礎，須在 `emfor-tcp-expect` archive 之後才開始實作。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 client 憑證（mTLS）、憑證到期前警告、STARTTLS 類協定升級、`http` 檢查的 TLS 實作改動。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `health-checking`：「TCP checker」需求新增 `tls`、`tls_verify`、`server_name`、`ca_file`，定義 TLS 握手、憑證驗證、與 `send` / `expect` 的搭配以及驗證錯誤。

## Impact

- 程式：新增 crates/emfor-tls；crates/emfor-daemon/src/checker/tcp.rs 支援 TLS；crates/emfor-check/src/net.rs 與 crates/emfor-check/src/options.rs 改用 emfor-tls；根目錄 Cargo.toml 的 workspace members。
- 相依：daemon 直接相依 rustls、tokio-rustls、rustls-native-certs、rustls-pki-types（都已在 Cargo.lock 中，因 reqwest 與 emfor-check 已使用），不新增套件。
- 文件：examples/emfor.toml、docs/spectra/config.yaml。
- 不影響 LMDB 格式、client、DNS 與外掛協定；沒設 `tls` 的 tcp 服務行為不變。
