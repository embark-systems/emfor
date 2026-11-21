## 1. 舊路徑的結果

- [x] 1.1 實作 spec peer-group "Peer secret reload" 的例外（設計決定「進行中的讀取記下路徑，結果回來時比對」）：`crates/emfor-daemon/src/peers/task.rs` 的 `ReadInFlight` 與 `SecretReader::refresh`。完成時：新單元測試（讀取函式先卡住、換路徑、放行並回傳一個有效的新密鑰）顯示該密鑰沒有被套用，而新路徑被讀；既有的 `changed_path_is_read_after_the_read_in_flight` 等測試仍通過或依新規則調整並說明。

## 2. 讀取上限

- [x] 2.1 實作 spec peer-group "Peer group configuration" 的上限（設計決定「上限 65536 位元組，用 `take` 讀」）：`crates/emfor-daemon/src/config.rs` 的 `read_secret`。完成時：新測試顯示 65536 位元組（加結尾換行）仍被接受、65537 位元組是錯誤、且讀取不超過 65537 位元組（用 `/dev/zero` 之類不會結束的檔案以外的方式，例如檢查回傳的錯誤；一般檔案即可）。

## 3. kqueue 開檔

- [x] 3.1 實作設計決定「kqueue 開檔用 `O_NONBLOCK`」：`crates/emfor-daemon/src/peers/watch.rs` 的 `kqueue_backend`（`Backend::new` 與 `register`）。完成時：在這台 macOS 上 `cargo check -p emfor-daemon` 與 `cargo clippy -p emfor-daemon -- -D warnings` 乾淨；Debian 的測試不受影響（這段不在 Linux 編譯）。

## 4. 文件

- [x] 4.1 [after: 1.1, 2.1, 3.1] `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md` 說明 `secret_file` 的大小上限；`CHANGELOG.md` Unreleased 加條目（Checked／Not verified；kqueue 未在 FreeBSD 實跑）。完成時：`python3 tools/check-manual-links.py` 報 0 個問題，`spectra validate` 通過。
