## Context

emfor-check 與 emfor-tls 讀憑證類檔案時，一律是 `std::fs::read(path)`：`read_password`、RADIUS 的 `read_secret`、`read_option_file`（驗證）、emfor-tls 的 `read_file_option`，以及 `FileCache` 的 `get_or_parse_at`（先 `std::fs::metadata(path)` 取 stamp，再 `std::fs::read(path)`）。沒有大小上限，也不檢查檔案型別。daemon 的 `config.rs` 的 `read_secret` 已經有完整的做法（`O_NONBLOCK` 開檔、對同一個 fd 取 metadata、`is_file()`、`take(MAX + 1)`），但只有聯防密鑰用到。

讀檔發生的位置：RADIUS 與各資料庫檢查在 emfor-check 行程的 async 函式裡讀 `password_file`，沒有 `spawn_blocking`；外層 `tokio::time::timeout` 加 `abort()` 對同步讀檔無效。http 與 tcp 檢查在 daemon 自己的 async worker 上經 `FileCache` 讀 `ca_file`。

依賴方向：emfor-tls 沒有 emfor-* 依賴；emfor-check 依賴 emfor-plugin 與 emfor-tls；emfor-daemon 依賴 emfor-client、emfor-format 與 emfor-tls。emfor-tls 是兩邊共有的 crate，也已經有 `libc`。

## Goals / Non-Goals

**Goals:**

- 憑證類檔案被指到 FIFO、裝置或目錄時，檢查回 error，不卡住任何執行緒。
- 檔案超過各選項的上限時，不整個讀進記憶體，回 error。
- emfor-check、emfor-tls 與 daemon 的 `read_secret` 共用同一個開檔、判斷型別、限制大小的實作。
- `FileCache` 的 stamp 與內容來自同一個 fd。

**Non-Goals:**

- 不做擁有者或權限拒絕（TODO 58 ① 與 67 項 4 已決定只警告，`credential_warnings` 不變）。
- 不改 RADIUS password 的 128 bytes 規則、AMQP 的 4096 bytes 規則、daemon peers 的 `SECRET_READ_WAIT` 與 `spawn_blocking`。
- 不把 daemon 內 http／tcp 的 `ca_file` 讀取搬進 `spawn_blocking`。
- 不改 daemon `read_secret` 的擁有者檢查、mode 檢查、`MIN_SECRET_LEN` 與錯誤訊息。

## Decisions

### 共用函式放在 emfor-tls，拆成「開檔」與「限量讀取」兩步

emfor-tls 新增兩個函式與一個錯誤型別：

- `open_regular(path: &Path) -> Result<(File, Metadata), FileError>`：以 `OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)` 開檔，對同一個 `File` 取 `metadata()`，`is_file()` 不成立就回 `FileError::NotRegular`。symlink 由 `open` 跟隨，檢查的是開出來的檔案，所以指向一般檔案的 symlink 通過。
- `read_limited(file: &mut File, max: u64) -> Result<Vec<u8>, FileError>`：`take(max + 1)` 讀取，長度超過 `max` 回 `FileError::TooLarge`。
- `FileError` 有三個值：`Io(io::Error)`、`NotRegular`、`TooLarge`。不含檔案內容。

拆成兩步的理由：daemon 的 `read_secret` 要在開檔與讀取之間，用同一個 `Metadata` 做擁有者與 mode 檢查。一個「開檔加讀完」的函式做不到，會再多一次 stat。

不選「把 daemon 的 `read_secret` 整個搬過來」：它包含擁有者與 mode 的拒絕，而 emfor-check 這邊明確不拒絕。也不選「emfor-check 依賴 emfor-daemon」：會讓函式庫 crate 依賴 daemon 這個大 crate。

### 各選項的上限

常數放在 emfor-tls，名稱與數值如下。數值是估計，不是量測：

| 選項 | 上限 | 依據 |
| --- | --- | --- |
| `password_file`、RADIUS `secret_file` | 64 KiB | 與 daemon 的 `MAX_SECRET_LEN` 一致；密碼與共享密鑰遠小於此 |
| `tls_key_file`、`tls_cert_file` | 1 MiB | 一把私鑰或一條憑證鏈約數 KB；1 MiB 留出很大餘裕 |
| `ca_file` | 8 MiB | 系統 CA bundle 估計約 200 KB，公司的 bundle 可能數倍；8 MiB 是保守值 |

確認估計的量測：apply 時量 Debian 容器內 `/etc/ssl/certs/ca-certificates.crt` 的實際大小並記進 CHANGELOG 的相容性說明；測試包含一個超過 64 KiB 但低於 8 MiB 的 `ca_file`，確認仍能載入。若實際 bundle 比 8 MiB 大，上限要調高，這個決定取決於上面還沒量的數字。

### `FileCache` 用同一個 fd 取 stamp 與內容

`get_or_parse_at` 改成：`open_regular` 一次，`FileStamp::of(&metadata)` 從該 fd 的 metadata 取；stamp 與快取相同就直接回傳快取值（`File` 在函式結束時關閉）；不同就用 `read_limited` 從同一個 `File` 讀，解析，存入快取。舊做法是 `metadata(path)` 與 `read(path)` 各自解析路徑，兩次之間檔案被換掉會讀到與 stamp 不符的內容。沿用既有註解的精神：stamp 在讀取之前取得，讀取期間的改變會讓下次檢查重讀。

快取命中時仍然開檔與 fstat 一次，比舊的 `metadata(path)` 多一次 open 與 close。每次檢查多一次，不是迴圈；沒有測量其成本，沿用「檢查的間隔以秒計」的判斷，不當成風險。

### 錯誤訊息保留各呼叫端現有格式

- emfor-check 的 `read_password`：`password_file: {kind}` 不變；新增 `password_file: not a regular file` 與 `password_file: file is too large (more than N bytes)`。RADIUS 的 `secret_file:` 同理。
- 驗證路徑 `read_option_file`、`read_file_option`：`file not found`、`cannot read: {kind}` 不變；新增 `not a regular file` 與 `file is too large (more than N bytes)`。
- `FileCache`：`{option}: {kind}` 不變；新增 `{option}: not a regular file` 與 `{option}: file is too large (more than N bytes)`。
- daemon `read_secret`：`cannot read {path}: {e}`、`{path} is not a regular file`、`{path} is too large (more than {MAX_SECRET_LEN} bytes)` 逐字不變，既有測試不改。

## Implementation Contract

**Behavior:**

- RADIUS `secret_file`、`password_file`，Diameter 的 `tls_cert_file`、`tls_key_file`，資料庫的 `password_file`，所有檢查的 `ca_file`：指到 FIFO、裝置或目錄時，檢查回 `error`（驗證時是選項錯誤），不卡住；超過上限時同樣回 error。一般檔案與指向一般檔案的 symlink 行為不變。
- daemon 的 `read_secret` 對外行為不變。

**Interface:**

- emfor-tls 公開 `open_regular`、`read_limited`、`FileError`，以及各選項的上限常數（名稱在 apply 時定，放在同一個 module）。
- `FileCache::get_or_parse` 的簽章不變。

**Failure modes:**

- FIFO 或裝置：`NotRegular`，不論有沒有寫入端都立即回傳（`O_NONBLOCK`）。
- 上限加一個 byte：`TooLarge`；剛好上限：通過。
- 錯誤訊息不含檔案內容。

**Acceptance criteria:**

- emfor-tls 單元測試：FIFO 被拒絕且在 5 秒內回傳、目錄被拒絕、剛好上限與上限加一個 byte、symlink 指向一般檔案通過、不存在的檔案回 `Io(NotFound)`。
- `FileCache` 測試：超過 64 KiB 的 `ca_file` 載入成功；FIFO 的 `ca_file` 得到 `ca_file: not a regular file`；既有的快取與換檔測試不改預期。
- emfor-check 測試：`read_password`、RADIUS `read_secret`、`read_option_file` 對 FIFO 與超大檔回 error。
- daemon：`config.rs` 既有的 `read_secret` 測試全部不改預期而通過。
- `tools/docker-test.sh` 全部通過。

**Scope boundaries:** 見 Goals / Non-Goals。mysql 的 `ssl_opts` 呼叫路徑沒追到底，apply 時要確認它是否經過 `FileCache` 或 `read_file_option`，沒經過的話改成經過，或在完成報告寫明。

## Risks / Trade-offs

- 拒絕 FIFO 是行為變更。systemd `LoadCredential`、Docker secrets、Kubernetes secrets 最後都是一般檔案，所以預期沒有現有安裝受影響，但這是推論，沒有逐一在這些環境測過；CHANGELOG 要寫相容性說明。
- 上限是估計。太低會讓合法的大型 `ca_file` 起不來；已用 8 MiB 保守設定並要求量測。
- `O_NONBLOCK` 開檔只防 FIFO 的開檔等待，不防「開得起來但讀很慢」的 NFS。後者仍會佔住 worker，列為 Non-Goals 的後續。
- daemon 的 `read_secret` 改用共用函式屬於重構，風險靠既有測試不改預期來控制。
