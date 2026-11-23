## 1. 共用的讀檔函式

- [x] 1.1 依 design「共用函式放在 emfor-tls，拆成開檔與限量讀取兩步」「各選項的上限」，在 `crates/emfor-tls` 新增 `open_regular`（`O_NONBLOCK` 開檔、對同一個 `File` 取 metadata、不是一般檔案回 `FileError::NotRegular`）、`read_limited`（`take(max + 1)`，超過回 `FileError::TooLarge`）、`FileError`（`Io`、`NotRegular`、`TooLarge`，不含內容）與各選項上限常數（password／secret 64 KiB、tls 憑證與私鑰 1 MiB、`ca_file` 8 MiB）。驗證：emfor-tls 單元測試，標註 `// spectra-scenario: <capability> / <scenario>` 者對應 spec：FIFO（無寫入端）在 5 秒內回 `NotRegular`、目錄回 `NotRegular`、剛好上限通過、上限加一個 byte 回 `TooLarge`、指向一般檔案的 symlink 通過、不存在的檔案回 `Io` 且 kind 為 NotFound；`tools/docker-test.sh 'cargo test -p emfor-tls'` 通過。

## 2. 改用共用函式

- [x] 2.1 [after: 1.1] 在 `crates/emfor-tls/src/lib.rs`，`read_file_option` 與 `FileCache::get_or_parse_at` 改用 `open_regular` 與 `read_limited`：`FileCache` 的 stamp 與內容來自同一個 `File`，stamp 不變就直接回快取值；錯誤訊息照 design「錯誤訊息保留各呼叫端現有格式」，新增 `not a regular file` 與 `file is too large (more than N bytes)`；失敗不進快取（維持現狀）。另外追 mysql 的 `ssl_opts`（`crates/emfor-check/src/mysql.rs`）是否經過 `FileCache` 或 `read_file_option`，沒經過就改成經過，或在完成時寫明。驗證：spec health-checking「ca_file of tcp and http checks is a bounded regular file」與 database-checks「Password and CA files of database checks are bounded regular files」的 scenario 各有測試：FIFO 的 `ca_file` 得到 `ca_file: not a regular file` 且不卡住、超過 64 KiB 且低於 8 MiB 的 `ca_file` 載入成功、超過 8 MiB 回 too large、錯誤之後換成有效檔案下次檢查讀新檔；既有的快取與換檔測試預期不改；`tools/docker-test.sh 'cargo test -p emfor-tls'` 通過。
- [x] 2.2 [after: 1.1] 在 `crates/emfor-check/src/options.rs` 與 `crates/emfor-check/src/radius.rs`，`read_password`、RADIUS 的 `read_secret`、`read_option_file` 改用 `open_regular` 與 `read_limited`（上限依選項）；`password_file: {kind}`、`secret_file: {kind}`、`file not found`、`cannot read: {kind}` 維持，新增 not a regular file 與 too large 兩種訊息，開頭仍是選項名稱。驗證：spec network-checks「Credential files of network checks are bounded regular files」與 database-checks「Password and CA files of database checks are bounded regular files」的 scenario 各有測試：RADIUS `secret_file` 是 FIFO 時檢查回 `error` 且原因以 `secret_file: ` 開頭、`password_file` 65537 bytes 驗證回 too large、剛好 65536 bytes 與 symlink 通過、Diameter `tls_key_file` 是目錄時驗證回 not a regular file；既有 password_file 測試不改預期；`tools/docker-test.sh 'cargo test -p emfor-check'` 通過。
- [x] 2.3 [after: 1.1] 在 `crates/emfor-daemon/src/config.rs`，`read_secret` 改用 `open_regular` 取得 `File` 與 `Metadata`，擁有者檢查、mode 檢查、`MIN_SECRET_LEN` 與結尾換行處理保留在原處，讀取改用 `read_limited(MAX_SECRET_LEN)`；三種錯誤訊息文字逐字不變。驗證：`config.rs` 與 `peers` 既有的 `read_secret` 相關測試全部不改預期而通過（FIFO、過大、擁有者、mode、symlink）；`tools/docker-test.sh 'cargo test -p emfor-daemon'` 通過。

## 3. 文件

- [x] 3.1 [after: 2.1, 2.2, 2.3] 手冊第 6 章（檢查選項）與第 13 章（安全）加入「憑證類檔案必須是一般檔案且有大小上限」的說明（en 與 zh_TW，zh_TW 照 `docs/spectra/LANGUAGE.md` 與自然用語改寫）：列出各選項上限、FIFO 與目錄會被拒絕、symlink 可以；`CHANGELOG.md` 的 Unreleased 加一條相容性說明，內容含 Debian 容器內 `/etc/ssl/certs/ca-certificates.crt` 的實際大小（apply 時量測，寫量到的數字）。驗證：內容審閱對照 design 的上限表與 spec；`python3 tools/check-manual-links.py` 通過。

## 4. 收尾

- [x] 4.1 [after: 3.1] 完整檢查：`tools/docker-test.sh`（不帶參數，含 fmt、clippy、全部測試）通過。驗證：docker-test.sh 回報全部通過。
