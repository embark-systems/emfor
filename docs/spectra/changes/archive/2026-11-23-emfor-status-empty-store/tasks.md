## 1. status 訊息

- [x] 1.1 實作 requirement「Status reads the store read-only」新增的空 store 句子。`crates/emfor-daemon/src/status.rs` 的 `Reader::read` 在 `meta/schema_version` 不存在、且沒有 `meta/heartbeat` 與 `svc/` 開頭的 key 時，回報 `store is empty: the daemon has not written to it yet; if it should be running, check its log`；有 heartbeat 或服務記錄但沒有版本時仍回報 `store format version (missing); this emfor reads versions 1 and 2`；版本 3 的訊息不變。驗證：與 `reader_reads_versions_1_and_2` 並列的單元測試涵蓋這三種 store，帶 `// spectra-scenario: status-command / Store without any data` 與 `... / Heartbeat but no format version` 註解；`tools/docker-test.sh` 通過。

## 2. 文件

- [x] 2.1 手冊第 16 章（`docs/manual/16-troubleshooting.md` 與 `docs/manual.zh_TW/16-troubleshooting.md`）的「daemon 無法啟動」表加一列 `store is empty: ...`：原因是 daemon 在第一次寫入前退出（例如 DNS port 被占用），處理是看 log；`CHANGELOG.md` 的 Unreleased 加一條。驗證：`python3 tools/check-manual-links.py` 回報 0 problems，並讀過中英兩列內容。

## 3. 檢查

- [x] 3.1 [after: 1.1, 2.1] 全部檢查通過：`tools/docker-test.sh`（fmt、clippy -D warnings、workspace 測試、doc tests）通過，`spectra analyze emfor-status-empty-store` 與 `spectra validate emfor-status-empty-store` 沒有 Critical 或失敗。
