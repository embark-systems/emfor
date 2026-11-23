## Why

emfor-check 和 emfor-tls 讀密碼類檔案（`password_file`、RADIUS 的 `secret_file`、`tls_key_file`、`tls_cert_file`、`ca_file`）時，全部是 `std::fs::read(path)`：沒有一般檔案檢查、沒有大小上限。路徑如果指到 FIFO 或卡住的 NFS，讀檔會卡住執行緒；外層的 `tokio::time::timeout` 蓋不住同步讀檔，`abort()` 也叫不醒它。

卡住的位置有兩處，嚴重程度不同：

- **emfor-check 行程**：RADIUS 和各資料庫檢查在另一個行程 `emfor-check` 裡跑。`read_password` 有 7 個 async 呼叫點（radius、amqp、ldap、mongodb、mysql、pgsql、redis），每次檢查都讀，沒有 `spawn_blocking`。卡住的是該行程的 worker。
- **daemon 本身**：http 與 tcp 檢查在 daemon 自己的 async worker 上經 `FileCache` 讀 `ca_file`（http 的 `client_for`、tcp 的 `handshake`），`FileCache` 還是先 `metadata(path)` 再 `read(path)`，兩次都各自解析路徑。卡住的是 daemon 的 worker。

daemon 的 `read_secret`（聯防密鑰）已經有 `O_NONBLOCK` 開檔、對同一個 fd `fstat`、只收一般檔案、`MAX_SECRET_LEN` 上限，0.7.0 修過 FIFO 讓每秒重讀卡住。同樣的防護沒有推到其他讀檔處。

這個變更只防「卡住」與「超大」。不做擁有者與權限拒絕：TODO 58 ① 與 67 項 4 已決定維持只警告（`credential_warnings`）。本機使用者視為可信任（TODO 23），實務風險低，目的是設定寫錯時不拖垮行程。

## What Changes

- emfor-tls 新增一組小函式：以 `O_NONBLOCK` 開檔、對同一個 fd 取 metadata、不是一般檔案就拒絕（FIFO、裝置、目錄；symlink 開檔後仍通過）、以 `take(max + 1)` 讀取並在超過上限時拒絕。錯誤只含路徑與原因，不含內容。
- emfor-check 的 `read_password`、RADIUS 的 `read_secret`、`read_option_file`，以及 emfor-tls 的 `read_file_option` 與 `FileCache`，改用這組函式。`FileCache` 改成從同一個 fd 取 stamp 與內容，消除 stat 與 read 之間的競態。
- daemon 的 `read_secret` 改用同一組函式做開檔、判斷一般檔案、讀取與上限；它的擁有者檢查、mode 檢查、`MIN_SECRET_LEN` 與錯誤訊息文字維持不變。
- 大小上限依選項分開。這些數字是估計，不是量測：`password_file` 與 RADIUS `secret_file` 64 KiB（與 daemon 的 `MAX_SECRET_LEN` 一致）；`tls_key_file`、`tls_cert_file` 1 MiB；`ca_file` 8 MiB。確認方式：量 Debian 上 `/etc/ssl/certs/ca-certificates.crt` 的實際大小（估計約 200 KB），並用一個超過 64 KiB 的 CA bundle 測試確認 `ca_file` 仍能載入。
- 相容性：拒絕 FIFO 與非一般檔案是行為變更。systemd `LoadCredential`、Docker secrets、Kubernetes secrets 最後都是一般檔案（Kubernetes 用 symlink，`fstat` 在開檔之後，仍通過）。CHANGELOG 寫相容性說明。
- 手冊第 6 章與第 13 章（en 與 zh_TW）寫明檔案型別與大小上限。

## Non-Goals

- 不對 emfor-check 的檔案做擁有者或權限拒絕，不改 `credential_warnings`。
- 不改 RADIUS password 的 128 bytes 規則、AMQP 的 4096 bytes 規則。
- 不改 daemon peers 的 `SECRET_READ_WAIT` 與 `spawn_blocking`。
- 不把 daemon 內 http 與 tcp 的 `ca_file` 讀取搬進 `spawn_blocking`。`O_NONBLOCK` 加一般檔案檢查之後不會再無限期卡住；慢速儲存（NFS 變慢但沒死）的讀取仍在 async worker 上，列為後續。
- 不改 `check_nul_free_password_file` 等驗證規則本身。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `network-checks`: RADIUS 的 `secret_file`、`password_file`，與 Diameter 的 `tls_cert_file`、`tls_key_file`，必須是一般檔案且不超過上限；否則在驗證與每次檢查時報錯，不卡住。
- `database-checks`: `password_file` 與 `ca_file` 同樣必須是一般檔案且不超過上限。
- `health-checking`: TCP 與 HTTP 檢查的 `ca_file` 同樣必須是一般檔案且不超過上限；FIFO 的 `ca_file` 讓檢查回 error，daemon 的 worker 不被卡住。

## Impact

- 程式：crates/emfor-tls/src/lib.rs（新增函式、`read_file_option`、`FileCache`）；crates/emfor-check/src/options.rs 與 crates/emfor-check/src/radius.rs（讀檔函式）；crates/emfor-daemon/src/config.rs（`read_secret`）。
- 規格：network-checks、database-checks、health-checking 各新增一條要求。peer-group 不動，行為維持不變。
- 文件：手冊第 6、13 章（en 與 zh_TW）、CHANGELOG Unreleased。
- 測試：FIFO 被拒絕且不卡住、剛好上限與上限加一個 byte、一般檔案、指向一般檔案的 symlink、超過 64 KiB 的 `ca_file` 仍可載入、daemon `read_secret` 既有測試全部不變。
- 沒驗證：mysql 的 `ssl_opts` 呼叫路徑沒追到底，apply 時要確認它是否走到 `FileCache` 或 `read_file_option`。
