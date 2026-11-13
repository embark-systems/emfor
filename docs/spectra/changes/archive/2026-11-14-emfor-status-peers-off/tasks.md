## 1. status 文字輸出

- [x] 1.1 crates/emfor-daemon/src/status.rs 的 `peer_lines` 在 `peers: N/M live` 之後、check definition 行之前，為 `services_off` 含顯示中服務的每個成員（依 `meta/peers` 順序）加一行 `peers: <成員> leaves out <服務...>`，服務只取顯示中的、依名稱排序（design「成員不參加的服務放在獨立的 peers 行」與「只列顯示中的服務」）。依 status-command「Status text output」。驗證：新單元測試重現情境「Members that leave services out」的行序與範例「leaves-out lines for member 10.0.0.6:5302」五列（含 `--service` 篩選與 daemon 停止）；既有的 `peers:` 測試不改仍通過；暫時拿掉新行時新測試失敗。

## 2. 文件與完整檢查

- [x] 2.1 docs/manual/08-operations.md 與 docs/manual.zh_TW/08-operations.md 的聯防群組一節說明新的 `peers: <成員> leaves out ...` 行，CHANGELOG.md 加一條。驗證：人工審閱與 status-command 規格一致。[after: 1.1]
- [x] 2.2 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 2.1]
