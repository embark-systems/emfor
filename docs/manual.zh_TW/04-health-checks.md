# 4. 健康檢查

[手冊](README.md) · 上一章：[3. 設定](03-configuration.md) · 下一章：[5. 選擇 backend](05-backend-selection.md)

服務的 `check` 決定如何測試每個 backend，`check_options` table 則設定該 checker。每次檢查都以 `up`、`down` 或 `error` 結束（[第 1 章](01-concepts.md#檢查結果與健康狀態)）。

| `check` | 測試什麼 | 適合 |
| --- | --- | --- |
| `tcp` | 一條 TCP 連線，可選擇加上 TLS 與 send/expect 交換 | 任何 TCP 服務；以歡迎訊息開場的協定，例如 SMTP |
| `http` | 一個 `GET` 請求與它的狀態碼 | Web 服務與 health endpoint |
| `exec` | 每次檢查執行一次的程式 | 既有的 Nagios 式 script、不頻繁的檢查 |
| plugin 名稱 | 長時間執行的 plugin 程式 | redis、MySQL、PostgreSQL、LDAP、DNS、RADIUS、SIP、Diameter（`emfor-check`），或你自己寫的 |

在服務的 `timeout` 內沒有完成的檢查，結果是 `down`，reason 為 `timeout`。唯一的例外是 plugin：plugin 沒有及時回答時結果是 `error`，因為遲到的是 plugin，不是 backend。

## tcp

沒有選項時，backend 接受連線即為 `up`；emfor 隨即關閉連線。連線被拒絕或無法到達則為 `down`。

| 選項 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `send` | 字串 | 無 | 連線後寫出一次的位元組。TOML 跳脫字元如 `\r\n` 會以對應位元組送出。需要搭配 `expect`。 |
| `expect` | 字串 | 無 | backend 送來的資料中必須出現的位元組（區分大小寫，最多讀 16384 位元組）。最長 16384 位元組。 |
| `tls` | 布林 | `false` | 連線後立即進行 TLS handshake；絕不退回明文。 |
| `tls_verify` | 布林 | `true` | `false` 接受任何憑證（仍然加密）。需要 `tls = true`。 |
| `server_name` | 字串 | backend IP | 憑證必須符合的名稱（或 IP）；是名稱時也作為 SNI 送出。需要 `tls = true`。 |
| `ca_file` | 絕對路徑 | 系統 CA | 改為信任這個 PEM 檔中的 CA 憑證。載入設定時檔案必須存在。需要 `tls = true`。 |

```toml
[services."smtp.svc.emfor"]
check = "tcp"
backends = ["10.0.6.11:25", "10.0.6.12:25"]
check_options = { expect = "220" }

[services."memcache.svc.emfor"]
check = "tcp"
backends = ["10.0.7.11:11211"]
check_options = { send = "stats\r\n", expect = "STAT pid" }

[services."imap.svc.emfor"]
check = "tcp"
backends = ["10.0.8.11:993"]
check_options = { tls = true, server_name = "imap.internal", expect = "* OK" }
```

常見的 reason：`expect: got "421 too busy\r\n"`（收到的前 64 位元組，已跳脫）、`send: ...`、`tls: ...`，以及 `tls: no answer to the TLS handshake (does the port speak TLS?)`。

## http

每次檢查對 backend 的 IP 與 port 送一個 `GET`。不會跟隨重新導向。

| 選項 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `scheme` | `"http"` 或 `"https"` | `"http"` | 協定。 |
| `path` | 以 `/` 開頭的字串 | `"/"` | 請求路徑。 |
| `host_header` | 字串 | 無 | 要送出的 `Host` header。 |
| `expect_status` | 非空清單 | `["200-399"]` | 視為 `up` 的狀態碼：單一狀態碼（`200` 或 `"200"`）或含頭尾的範圍（`"200-299"`），都必須在 100–599 之間。 |
| `tls_verify` | 布林 | `true` | 用於 `https`：`false` 接受任何憑證。 |

```toml
[services."api.svc.emfor".check_options]
path = "/healthz"
host_header = "api.internal"
expect_status = ["200-299"]
```

狀態碼不在清單內時為 `down`，reason 中帶有狀態碼。

## exec

每次檢查執行一次程式，不經過 shell。backend 以環境變數 `EMFOR_IP`、`EMFOR_PORT` 與 `EMFOR_SERVICE` 傳入。

```toml
[services."report.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/check_pg_replication", "--max-lag", "60"] }
```

| 結束方式 | 結果 |
| --- | --- |
| 0 | `up` |
| 1 或 2 | `down`；reason 為 stdout 的第一行，或 `exit code N` |
| 其他結束代碼、被 signal 終止、無法啟動 | `error` |
| 到 `timeout` 仍在執行 | `down`（`timeout`）；程式與它所有子行程都會被終止 |

`command` 是唯一的選項：非空清單，第一個元素是可執行檔的絕對路徑。每次檢查都啟動一個行程的成本不低；需要頻繁檢查很多 backend 時，請用 plugin。

## plugin 與 emfor-check

plugin 是長時間執行的程式：daemon 啟動它一次，之後透過它的 stdin 與 stdout，以 JSON 行傳送檢查請求。一個行程服務所有使用它的服務，並且可以在檢查之間保留連線與狀態。

emfor 附帶一個 plugin 程式 `emfor-check`，由參數選擇檢查種類：

```toml
[plugins."pgsql"]
command = ["/usr/local/libexec/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/pg.pass" }
```

| 參數 | 何時為 up |
| --- | --- |
| `redis` | `PING` 回答 `PONG`（設定密碼時先 `AUTH`） |
| `mysql` | 登入成功且 `SELECT 1` 回傳 1（以 MariaDB 測試） |
| `pgsql` | 登入成功且 `SELECT 1` 回傳 1 |
| `ldap` | simple bind 成功 |
| `dns` | 一次查詢得到預期的 response code（設定 `expect_answer` 時還要有記錄） |
| `radius` | Status-Server（或 Access-Request）得到以共享密鑰簽署的 accept |
| `sip` | OPTIONS 請求得到 `expect_status` 內的最終狀態碼（預設 200 到 499） |
| `diameter` | 能力交換（CER / CEA）與 Device-Watchdog（DWR / DWA）成功 |

主要選項：`user`、`password_file`（絕不寫密碼本身；每次檢查都會讀取）、`database`（`pgsql`）、`role`（redis 為 `master`/`replica`，MySQL 與 PostgreSQL 為 `primary`，讓 `failover` 跟著 primary 走），以及 `tls`、`starttls`（`ldap`）、`tls_verify`、`server_name`、`ca_file`。`dns` plugin 則用 `name`、`type`、`transport`、`recursion_desired`、`expect_rcode`、`expect_answer`；`radius` plugin 用 `secret_file`、`request`、`application`、`user`、`password_file`、`reject_is_up`、`nas_identifier`、`require_message_authenticator`；`sip` plugin 用 `transport`、`tls`（及 TLS 相關鍵）、`uri`、`from`、`expect_status`；`diameter` plugin 用 `origin_host`、`origin_realm`、各 application id、`supported_vendor_ids`、`host_ip_address`、`tls`（及 TLS 相關鍵與客戶端憑證 `tls_cert_file`、`tls_key_file`）。[docs/deploy/plugins.md](../deploy/plugins.md) 列出所有選項與 reason。

`emfor check-config` 與每次 reload 都會啟動 plugin，讓它們驗證每個服務的 `check_options`，所以錯誤會在設定套用前就被發現。

plugin 結束時，等待中的檢查變成 `error`（`plugin exited`），daemon 在 1 秒、2 秒、4 秒……最多 30 秒後重新啟動它；在那之前它的檢查都是 `error`（`plugin <name> unavailable`）。error 不改變 backend 狀態，所以當掉的 plugin 不會讓 backend 變成 down。plugin 寫到 stderr 的每一行都以 `info` 記錄。

要自己寫 plugin，見 [docs/plugins/protocol-v1.md](../plugins/protocol-v1.md)；`emfor-plugin` crate 為 Rust 實作了這個協定。

## 排程

每個 backend 依自己的時程檢查，而且同一個 backend 不會同時有兩個檢查：

- 啟動或 reload 後的第一次檢查，在 `min(interval, 200ms)` 內的隨機時刻開始。
- backend 處於 `unknown` 且檢查結果為 `up` 或 `down` 時，下一次檢查在上一次結束後 `min(interval, 200ms)` 開始。以預設值（`rise = 2`），約半秒內就能確定 backend 狀態。
- 其他情況下，下一次檢查在上一次結束後 `interval` 開始，並隨機前後偏移最多 `jitter`（預設 10%）。

因為 interval 從檢查結束時算起，慢的 backend 不會比每 `interval` 一次更頻繁地被檢查。

## rise、fall 與 error

- `up`：成功計數加一，失敗計數歸零；到達 `rise` 時 backend 變成 `healthy`。
- `down`：失敗計數加一，成功計數歸零；到達 `fall` 時 backend 變成 `unhealthy`。
- `error`：計數與狀態都不變。backend 只收到 error 的時間超過 `fall × interval` 時，回到 `unknown`。

偵測故障大約需要 `fall × interval`（預設 6 秒）；恢復大約需要 `rise × interval`（4 秒）。每次狀態變化都以 `info` 記錄，包含服務、backend、舊狀態、新狀態與 reason。

## reason

`down` 或 `error` 結果帶有一個 reason，會顯示在 `emfor status`、log 與狀態歷史中。儲存前 reason 會被處理成安全的形式：tab、CR 與 LF 變成 `\t`、`\r` 與 `\n`；其他控制字元變成 `\u{1b}` 之類的形式；超過 512 個字元的 reason 會被截斷並以 `…` 結尾。

## 延遲

daemon 為每個 backend 量測每次 `up` 檢查花了多久，並保存一個平滑後的值（每個新樣本占 5%）。`emfor status` 與 `/metrics` 會顯示它，`balance = "fastest"` 會用到它。依賴這個值時，請選擇接近實際流量的檢查。
