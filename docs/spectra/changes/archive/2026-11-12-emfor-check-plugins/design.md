## Context

emfor-plugins 定好了外部外掛的協定（checker-plugins 規格、protocol 1）：daemon 對每個 `[plugins."<名稱>"]` 起一個常駐子程序，逐行 JSON 交換 `hello`、`validate`、`check`，`check` 帶 `service`、`target`、完整的 `options`（daemon 用 `crates/emfor-daemon/src/plugin/protocol.rs` 的 `options_to_json` 從 TOML 轉成 JSON）與 `timeout_ms`。目前 repo 裡唯一的外掛是測試用的 `emfor-test-plugin`（`crates/emfor-daemon/tests/support/test_plugin.rs`），沒有可以實際部署的外掛。

daemon 的 http 檢查器透過 reqwest 使用 rustls，Cargo.lock 已有 rustls 0.23、tokio-rustls 0.26、rustls-native-certs、aws-lc-rs 與 ring。設定慣例：http 檢查器的憑證驗證開關叫 `tls_verify`（預設 true）；不認得的選項報 `unknown key`。

驅動確認結果（task 1.1，2026-11-12，讀 crate 原始碼確認）：

- 版本：mysql_async 0.37.1（`default-features = false`，features `minimal-rust`、`rustls-tls`）、tokio-postgres 0.7.18、tokio-postgres-rustls 0.14.0、rustls 0.23（Cargo.lock 已有 0.23.46）、rustls-native-certs 0.8、測試用 rcgen 0.14。PEM 解析用 rustls-pki-types 的 `pem` 模組，不另加 rustls-pemfile。
- 加密後端：daemon 的 rustls 經 reqwest 使用 aws-lc-rs；三個驅動都跟著 rustls 的預設（aws-lc-rs），不另引入 ring。daemon 本身沒有使用 rustls-native-certs（reqwest 走 rustls-platform-verifier），emfor-check 會自己加入它來讀系統 CA。
- mysql_async：`SslOpts` 有 `with_danger_tls_hostname_override`（`server_name`）、`with_root_certs`（接受 PEM 或單張 DER 的 `PathOrBuf`）、`with_disable_built_in_roots`、`with_danger_accept_invalid_certs`（`tls_verify = false`）。沒設覆寫名稱時，它以 `ServerName::try_from(ip)` 比對 IP SAN。內建的根憑證是 webpki-roots 而非系統 CA，所以沒設 `ca_file` 時改為關閉內建根憑證、把 rustls-native-certs 讀到的系統憑證逐張以 DER 傳入；不接受自訂 `ClientConfig`，因此 mysql 的 TLS 由這些選項組出與共用 ClientConfig 相同的語意。
- tokio-postgres：`MakeRustlsConnect::new(ClientConfig)` 接受共用的 ClientConfig；`hostaddr` 設後端 IP、`host` 設 `server_name`（沒設時為 IP 字串），TLS 名稱與 SNI 取自 `host`；`ssl_mode = Require` 確保不退回明文，憑證驗證由 ClientConfig 負責。
- 結論：設計決策「TLS 用 rustls，一份共用的 ClientConfig 建構」可行，mysql 以 SslOpts 對應，不需要變更規格。

本機驗證流程已有 `tests/resolver/run.sh`：用 Dockerfile 建映像，在容器裡跑真的 dnsmasq／unbound。這次的實機測試照同樣模式。

## Goals / Non-Goals

**Goals:**

- 提供 `emfor-check` 執行檔，內含 `redis`、`mysql`、`pgsql` 三個外掛，完全遵守 protocol 1。
- 能判斷「服務活著且可登入使用」，並可選擇只把主庫（或指定角色）判為健康。
- 密碼不出現在設定檔、檢查原因與 log。
- TLS 與憑證驗證，行為和 http 檢查器的 `tls_verify` 一致。
- 提供 `emfor-plugin` crate，讓 Rust 外掛只需實作「驗證選項」與「做一次檢查」。
- 三種服務各有真實伺服器的自動化測試，含 TLS。

**Non-Goals:**

- HAProxy 式 agent check（後端回報 drain、權重）：protocol 1 的結果只有 up／down／error，需要改協定與 store，另開 change。
- smtp、ldap、ssl-hello 等其他協定外掛；內建 tcp 檢查器的 send／expect。
- 讓 daemon 改用 `emfor-plugin` 裡的訊息型別：daemon 程式碼這次不動。
- 連線池或跨檢查重用連線。
- 從設定檔或環境變數讀明碼密碼。
- 打包（deb、pkg）與安裝程式；文件只說明放置路徑。
- 在 FreeBSD 與真正的 Oracle MySQL 上的實機驗證（見 Risks）。

## Decisions

### 一個 emfor-check 執行檔，以第一個參數選外掛

`emfor-check redis|mysql|pgsql`。參數缺少、多於一個或不認得時，往 stderr 印用法並以結束碼 2 結束；daemon 因此握手失敗，設定被拒絕，錯誤訊息裡有外掛名稱（checker-plugins 既有行為）。`hello` 回覆的 `name` 是外掛名稱（`redis` 等），`version` 是 crate 版本。

替代方案：三個執行檔。建置、安裝、文件都多三倍，而共用的部分（協定、TLS、password_file）佔大部分程式碼，因此不採用。

### emfor-plugin crate 處理 protocol 1

新 crate `crates/emfor-plugin`（library）。外掛實作一個 trait：

- `name()`：hello 用的名稱。
- `validate(&options) -> Vec<OptionError>`：同步、純驗證，可讀檔（password_file、ca_file）。
- `check(CheckRequest) -> CheckOutcome`：async；`CheckRequest` 含 `service`、`ip`、`port`、`options`、`timeout`。

`emfor_plugin::run(plugin)` 負責：建立 tokio runtime、讀 stdin 每一行、回 hello（protocol 不是 1 時回 hello 後以結束碼 1 結束）、每個 check 開一個 task 並以 `timeout_ms` 包住（逾時回 `down`／`timeout`）、經單一 writer 寫 stdout（一行一個回應，不交錯）、stdin EOF 時等進行中的檢查結束再退出（最多等到它們各自的 timeout）。不認得 `type` 的行往 stderr 印一行並忽略。

替代方案：每個外掛自己處理協定。重複且容易各自出錯，不採用。

### 檢查內容與 role 對應

| 外掛 | 檢查 | `role` 值 | 角色判斷 |
| --- | --- | --- | --- |
| redis | 若有 `user` 或 `password_file`：`AUTH`；接著 `PING` 必須回 `+PONG` | `master`、`replica` | `ROLE` 第一個元素：`master` 對 `master`；`slave` 對 `replica`；其他（如 `sentinel`）皆不符 |
| mysql | 以 `user`（與密碼）登入，`SELECT 1` 回 1 | `primary` | `SELECT @@global.read_only` 為 0 |
| pgsql | 以 `user`、`database`（與密碼）登入，`SELECT 1` 回 1 | `primary` | `SELECT pg_is_in_recovery()` 為 false |

三個外掛都用 `user` 作帳號選項名稱（redis 文件稱 username，但跨外掛一致較好記）。redis 的 `AUTH` 有 `user` 時送 `AUTH <user> <password>`，只有密碼時送 `AUTH <password>`；只有 `user` 沒有 `password_file` 是驗證錯誤（redis 不支援只給帳號）。

### 密碼只用 password_file，每次檢查重新讀取

`password_file` 必須是絕對路徑。validate 時讀一次：不存在、不可讀、去掉結尾的 `\n` 或 `\r\n` 後為空，都是該鍵的驗證錯誤。check 時每次重新讀，讓密碼輪替不必 reload；這時讀不到是 `error`（外掛這端的問題，不算後端失敗），原因寫 `password_file: <io 錯誤種類>`，不含內容。任何原因字串、stderr 輸出都不包含密碼內容；驅動回傳的錯誤訊息在寫入原因前不做額外處理，因為 MySQL、PostgreSQL、redis 的認證失敗訊息不回顯密碼。

### TLS 用 rustls，一份共用的 ClientConfig 建構

共用模組依選項建 `rustls::ClientConfig`：

- `ca_file` 有設：只信任該 PEM 檔內的憑證（validate 時檢查絕對路徑、可讀、至少有一張憑證）；沒設：系統 CA（rustls-native-certs）。
- `tls_verify = false`：接受任何憑證（仍加密）。
- `server_name`：比對憑證與 SNI 用的名稱；沒設時用後端 IP（憑證需有 IP SAN）。

`tls = false` 時設了 `tls_verify`、`server_name` 或 `ca_file` 是驗證錯誤（`requires tls = true`），避免以為有加密其實沒有。`tls = true` 時 TLS 握手或憑證驗證失敗為 `down`，原因以 `tls: ` 開頭，不退回明文。加密後端用 Cargo.lock 中 daemon 已使用的那一個，不另外引入第二個。

各驅動接法：redis 自己用 tokio-rustls 包 TCP；pgsql 用 tokio-postgres 搭配以同一份 ClientConfig 建的 rustls connector，`hostaddr` 為後端 IP、`host` 為 `server_name`；mysql 用 mysql_async 的 rustls 支援。mysql_async 能否接受自訂的 server name 與 ClientConfig 未確認，是 task 1.1 的第一個驗證項目。

### redis 自己實作 RESP 而不用 redis crate

只需要 `AUTH`、`PING`、`ROLE` 三個指令，回應型別只有 simple string、error、bulk string、array、integer；自寫 RESP2 編解碼的估計規模約 150 行（估計，實作後以行數確認），而 redis crate 會帶進連線管理、cluster 等用不到的東西，且其 TLS 設定不易插入自訂 ClientConfig。

### 每次檢查一條新連線

連線、（TLS）、認證、查詢、關閉都在一次檢查內完成，整段受 `timeout_ms` 限制。重用連線會讓「新連線能不能建立」測不到（例如 max_connections 滿了），而 interval 秒級的頻率下建連成本不重要。

### 結果分類

- `up`：全部步驟成功且角色相符。
- `down`：連不上、逾時、TLS 失敗、認證失敗、查詢失敗、回應不對、角色不符。原因簡短且固定開頭：`connect: `、`timeout`、`tls: `、`auth: `、`query: `、`role is <實際>, want <設定>`。
- `error`：外掛本身無法完成檢查而與後端無關：password_file 或 ca_file 在檢查時讀不到、本機資源不足（EMFILE、ENFILE、ENOMEM）。

### 實機測試用一個 Debian 映像、三種伺服器

`tests/plugins/run.sh` 照 `tests/resolver/run.sh` 的模式：建 `emfor-plugins-test` 映像（Debian 套件的 redis-server、mariadb-server、postgresql、openssl），容器內以 openssl 產生測試 CA 與兩張伺服器憑證（一張名稱正確、一張名稱錯），各伺服器開明文與 TLS 埠，再以 Rust 整合測試（`#[ignore]`，由腳本以 `--ignored` 執行）透過 protocol 1 直接驅動 `emfor-check`，並跑一次真實的 `emfor` daemon 讀取 `failover` 服務。MySQL 端以 MariaDB 代替，原因見 Risks。

## Implementation Contract

**執行檔與設定**

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-check", "redis"]

[services."cache.svc.emfor"]
check = "redis"
balance = "failover"
backends = ["10.0.0.11:6379", "10.0.0.12:6379"]
check_options = { role = "master", password_file = "/etc/emfor/redis.pass", tls = true, server_name = "redis.internal", ca_file = "/etc/emfor/ca.pem" }
```

**選項（各外掛共通）**

| 鍵 | 型別 | 預設 | 驗證錯誤 |
| --- | --- | --- | --- |
| `user` | 非空字串 | 無（mysql、pgsql 必填） | `missing required key`、`must be a non-empty string` |
| `password_file` | 絕對路徑字串 | 無 | `not an absolute path`、`file not found`、`cannot read: <原因>`、`file is empty` |
| `role` | 字串 | 無（不檢查角色） | `must be one of master, replica`（redis）、`must be primary`（mysql、pgsql） |
| `tls` | 布林 | false | `must be a boolean` |
| `tls_verify` | 布林 | true | `must be a boolean`、`requires tls = true` |
| `server_name` | 非空字串 | 後端 IP | `must be a non-empty string`、`requires tls = true` |
| `ca_file` | 絕對路徑字串 | 系統 CA | `not an absolute path`、`file not found`、`cannot read: <原因>`、`no certificate found`、`requires tls = true` |
| `database` | 非空字串 | 無（pgsql 必填；redis、mysql 不接受） | `missing required key`、`unknown key` |

其他鍵一律 `unknown key`。redis 有 `user` 無 `password_file` 時，錯誤鍵為 `user`：`requires password_file`。

**行為**：見 Decisions 的「檢查內容與 role 對應」與「結果分類」；規格情境在 `specs/database-checks/spec.md`。

**驗收**

- `cargo test -p emfor-plugin -p emfor-check`：協定迴圈（hello、亂序回覆、逾時回 down、EOF 結束）、每個外掛的選項驗證表、password_file 各錯誤、RESP 編解碼、結果分類。
- `tests/plugins/run.sh`：三種伺服器各驗 up、密碼錯 down、伺服器停掉 down、角色不符 down、TLS 憑證正確 up、名稱不符 down、`tls_verify = false` 時名稱不符仍 up；daemon 端 failover 服務選到主庫。
- macOS 與 Debian 映像的 fmt、clippy、test 照常通過；daemon 測試不受影響。

**範圍**：只新增 `crates/emfor-plugin`、`crates/emfor-check`、`tests/plugins`、文件與範例設定；daemon、client、store 格式不變。

## Risks / Trade-offs

- [mysql_async 可能無法設定自訂 server name 或 ClientConfig] → task 1.1 先以最小程式確認；不行時改用 mysql_async 提供的選項組合（root 憑證、略過名稱驗證）達成相同語意，若仍做不到 `server_name`，在規格與文件中寫明 mysql 的限制並回報使用者決定。
- [實機測試以 MariaDB 代替 Oracle MySQL] → Debian 只提供 MariaDB；兩者都支援 `SELECT 1` 與 `@@global.read_only`。文件註明只以 MariaDB 驗證；真正 MySQL 8 的驗證留給使用者環境。
- [新相依套件讓 `emfor-check` 體積與建置時間增加] → 只影響 `emfor-check`，daemon 與 libemfor 不連結這些套件；以 `cargo tree -p emfor-daemon` 確認沒有混入。
- [每次檢查新連線對資料庫造成負載] → 每台後端每個 interval 一次連線，與一般監控相同量級；文件提醒 interval 不要設太短，並建議為 emfor 建專用低權限帳號。
- [FreeBSD 未實機驗證] → 和其他元件一樣，文件標明。

## Migration Plan

新增功能，不影響既有設定。部署：安裝 `emfor-check` 到如 `/usr/local/libexec/`，建立密碼檔（建議 0600，擁有者為執行 emfor 的帳號），在設定加入 `[plugins.*]` 並把服務的 `check` 改成外掛名稱後 reload。回復：把 `check` 改回 `tcp` 並 reload。

## Open Questions

（無）mysql_async 的 TLS 能力由 task 1.1 確認，處理方式已寫在 Risks。
