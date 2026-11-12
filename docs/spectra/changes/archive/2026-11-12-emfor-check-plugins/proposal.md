## Why

emfor-plugins 做好了外部外掛的架構（`[plugins.*]`、protocol 1），但 emfor 本身沒有附任何外掛。要檢查 redis、MySQL、PostgreSQL 這類最常見的後端，現在只能用 tcp（只知道 port 有開）、exec（每次檢查起一個程序、自己寫腳本），或自己從頭實作 protocol 1。這三種服務也常見主從架構，搭配 `balance = "failover"` 時，需要的是「只有主庫算健康」，tcp 和 http 都判斷不了。和 HAProxy 比較後（它內建 redis、mysql、pgsql 檢查），決定把這三種做成 emfor 自帶的外掛。

## What Changes

- 新執行檔 `emfor-check`：一個執行檔內含三個外掛，第一個參數選外掛，例如 `[plugins."redis"] command = ["/usr/local/libexec/emfor-check", "redis"]`。
  - `redis`：送 `PING`，收到 `PONG` 才算 up；可設 `username` 與 `password_file`（`HELLO`/`AUTH`）。
  - `mysql`：以 `user`（必填）登入後執行 `SELECT 1`。
  - `pgsql`：以 `user`、`database`（皆必填）登入後執行 `SELECT 1`。
  - 選填 `role`：redis 為 `master` 或 `replica`（`ROLE` 指令）；mysql 與 pgsql 為 `primary`（mysql 檢查 `@@read_only = 0`，pgsql 檢查 `pg_is_in_recovery()` 為 false）。角色不符為 down。
  - 密碼只能用 `password_file` 提供，不接受寫在設定檔的明碼；validate 時檢查檔案存在、可讀、去掉結尾換行後不是空的；密碼不會出現在檢查原因或 log。
  - TLS：`tls`（預設 false；true 時一定走 TLS，握手失敗為 down，不退回明文）、`tls_verify`（預設 true，意義同 http 檢查器）、`server_name`（預設為後端 IP，用於憑證比對與 SNI）、`ca_file`（預設系統 CA）。全部使用 rustls。
  - 每次檢查開一條新連線，檢查完關閉。
- 新函式庫 crate `emfor-plugin`：處理 protocol 1 的握手、validate、check 與 id 對應，外掛作者只要實作驗證與檢查兩個函式。`emfor-check` 用它；其他人用 Rust 寫外掛也可以直接用。
- 實機測試：容器內跑真的 redis、MySQL、PostgreSQL（含 TLS 埠，CA 與憑證在測試時產生），由 daemon 透過 `emfor-check` 檢查，納入本機檢查流程，和 `tests/resolver/run.sh` 並列。
- 文件：範例設定、外掛使用說明（含 `password_file` 權限建議）。

## Capabilities

### New Capabilities

- `database-checks`：`emfor-check` 執行檔與其 redis、mysql、pgsql 外掛：選項、檢查內容、角色檢查、`password_file`、TLS，以及各種結果（up、down、error）與原因。

### Modified Capabilities

（無）外掛協定與監管行為不變，新執行檔只是遵守該協定的外掛。

## Impact

- Affected specs: database-checks（新增）。
- Affected code: Cargo.toml（workspace members）、crates/emfor-plugin（新）、crates/emfor-check（新）、tests/plugins（新：容器實機測試）。daemon 程式碼不變。
- 新增相依套件（只進 `emfor-check`，不進 daemon 與 client）：mysql_async、tokio-postgres、tokio-postgres-rustls、tokio-rustls、rustls-pemfile 或同等的 PEM 解析。實作時確認各版本支援 rustls 並記入 Cargo.lock。測試另用 rcgen（dev-dependency）產生 TLS 測試憑證。
- Affected docs: examples/emfor.toml、docs/deploy/（新增外掛說明）、docs/spectra/config.yaml。
- 新 Docker 映像（實機測試用）：建立前先徵求使用者同意。
