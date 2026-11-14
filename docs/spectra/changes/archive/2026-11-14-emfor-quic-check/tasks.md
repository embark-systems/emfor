## 1. 依賴與 plugin 骨架

- [x] 1.1 依決策「用 quinn 與 h3／h3-quinn，鎖定確切版本」，在 `crates/emfor-check/Cargo.toml` 加入 `quinn`（只開 rustls 與 tokio runtime 相關 feature，使用 repo 現有的 rustls 0.23 與 aws-lc-rs）、`h3`、`h3-quinn`，三者都用 `=` 鎖定版本並更新 `Cargo.lock`。加入前後各量一次 `cargo build --release -p emfor-check` 的 `emfor-check` 大小並記下。驗證：macOS 與 Debian 容器 `emfor-clients-test` 都能 `cargo build -p emfor-check`；`cargo tree -p emfor-daemon` 不含 quinn 與 h3。
- [x] 1.2 [after: 1.1] 依 database-checks requirement「Plugin selection by argument」與 network-checks requirement「QUIC check options」，在 `crates/emfor-check/src/lib.rs` 加入 `Kind::Quic`（參數 `quic`，`hello` 的 name 為 `quic`），新增 `crates/emfor-check/src/quic.rs` 的選項解析，並更新 usage 與 `options.rs` 中列舉所有 `Kind` 的 match。檢查函式先回 `error`「not implemented」，由 2.1 取代。驗證：
  - `quic.rs` 的選項表格測試涵蓋 Example「QUIC option validation」每一列；
  - `crates/emfor-check/tests/cli.rs` 新增 `quic` handshake 測試（Scenario「QUIC plugin handshake」）；
  - 暫時拿掉 `requires http3 = true` 的檢查時表格測試失敗。

## 2. 檢查行為

- [x] 2.1 [after: 1.2] 依 requirement「QUIC check」規則 1 到 7 與決策「每個 address family 共用一個 quinn endpoint」「ALPN 不符就是 TLS 失敗」，實作只握手的檢查：共用 endpoint（`OnceCell`，綁定失敗回 `error` 並在下次重試）、用 `emfor_tls` 的設定建立 QUIC client 設定、握手、檢查協商出的 ALPN、以 code 0 關閉。reason 依 requirement 的文字。驗證：`quic.rs` 以 in-process quinn server 與 `emfor_tls::testing` 憑證測試 Example「QUIC and HTTP/3 outcomes」中 `http3` 未開的每一列（握手 up、ALPN 不符、不受信任的憑證、`tls_verify = false`、`server_name` 不符、server 以 0x10 與 `busy` 關閉、沒人回應的 UDP port 逾時），以及 Scenario「Nothing answers on the UDP port」在約 `timeout_ms` 內回答；每個測試都先確認拿掉對應行為時會失敗。
- [x] 2.2 [after: 2.1] 依 requirement「QUIC check」規則 8 到 11 與決策「HTTP/3 選項沿用 http check 的語意」，以 `h3`／`h3-quinn` 實作 `http3 = true`：`GET path`、`:authority` 的選擇、`user-agent`、不跟隨 redirect、`expect_status`、`expect_body`（最多讀 65536 bytes，找到即停）、以 `H3_NO_ERROR` 關閉。驗證：in-process h3 server 測試 Example 中 `http3 = true` 的每一列（200、302、503、body 符合與不符、503 加 body、`host_header` 成為 `:authority`、stream 被 reset），每個測試都先確認拿掉對應行為時會失敗。

## 3. 實機測試

- [x] 3.1 [after: 2.2] 在 `tests/plugins/Dockerfile` 加入一個 HTTP/3 server：先確認 Debian bookworm 的 `caddy` 套件存在且預設開啟 HTTP/3；不行就改用 `python3-aioquic` 的範例 server；兩者都不行就停下回報，不跳過。在 `tests/plugins/inside.sh` 啟動它（使用該腳本已產生的 server 憑證），並在 `crates/emfor-check/tests/live.rs` 新增 `quic` 測試：只握手 up、`http3 = true` 加 `path` 的 200 up、不存在的 path（404）down。同步更新 `tests/plugins/run.sh` 與 Dockerfile 開頭註解的 plugin 清單。驗證：`tests/plugins/run.sh` 全部通過。

## 4. 文件與發布紀錄

- [x] 4.1 [after: 1.2] 說明 `quic` plugin（英文與中文內容一致）：
  - `docs/manual/04-health-checks.md` 與 `docs/manual.zh_TW/04-health-checks.md`：plugin 表格與「Tests」表格各加一列，列出它自己選項的段落加入 `quic`；
  - `docs/deploy/plugins.md`：新增 `quic` 一節，包含選項表、reason 的意義，以及 UDP 沒有 connection refused、port 沒開與防火牆都是 `timeout`、daemon 主機要能送 UDP 到 backend；
  - `examples/emfor.toml`：加入註解掉的 `[plugins."quic"]` 與一個 `check = "quic"` 服務範例；
  - `fuzz/fuzz_targets/plugins.rs` 開頭註解註明 `quic` 不在其中（加密的 QUIC 無法由該 harness 餵入）。

  驗證：人工對照兩份 delta spec；`emfor check-config` 讀取 `examples/emfor.toml`，以及把 quic 範例註解取消後的版本，都通過。
- [x] 4.2 [after: 4.1] 在 `CHANGELOG.md` 未發布區段的 emfor-check 小節加入 `quic` plugin 的條目，包含 1.1 量到的 `emfor-check` 大小變化。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 5. 整體檢查

- [x] 5.1 [after: 2.2, 3.1, 4.2] 依 RELEASING.md，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`；並以 `-max_total_time=60` 跑 `plugins`、`plugin_line` 與 `config` fuzz target。驗證：所有指令 0 個錯誤、0 個失敗，`fuzz/artifacts/` 沒有新檔。
