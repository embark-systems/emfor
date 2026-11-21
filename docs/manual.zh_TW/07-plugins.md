# 7. Plugin 與 emfor-check

[手冊](README.md) · 上一章：[6. 健康檢查](06-health-checks.md) · 下一章：[8. 如何選出 backend](08-choosing-a-backend.md)

**plugin** 是長時間執行的程式：daemon 啟動它一次，之後把檢查請求送給它。一個行程服務所有使用它的服務，並且可以在檢查之間保留狀態。emfor 附帶一個 plugin 程式 `emfor-check`，內含十三種檢查；你也可以[自己寫](#自己寫-plugin)。

## 使用 plugin

在 `[plugins]` 下宣告 plugin，再把它的名稱寫成服務的 `check`：

```toml
[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }
```

- plugin 的名稱是 1–63 個由 `a-z`、`0-9`、`_` 與 `-` 組成的字元，而且不能是 `tcp`、`http` 或 `exec`。它不必與參數相同：名為 `pg-main` 的 plugin 可以執行 `emfor-check pgsql`。
- `command` 是程式與它的參數，不經 shell 執行。第一個元素是可執行檔的絕對路徑。`emfor-check` 在套件中是 `/usr/bin/emfor-check`，在 FreeBSD port 中是 `/usr/local/bin/emfor-check`；從原始碼建置時，它在你安裝它的地方（[第 4 章](04-installing.md#從原始碼建置)）。
- `env_pass` 列出 plugin 在 `PATH`、`LANG`、`TZ` 與 `LC_*` 之外還拿到的變數（[第 6 章](06-health-checks.md#檢查程式拿到的環境)）。
- 每個宣告的 plugin 都會啟動，不論有沒有用到。
- port 一律來自 backend 位址；沒有任何 plugin 有 `port` 選項。

### daemon 如何處理 plugin

| 何時 | 發生什麼 |
| --- | --- |
| 啟動、reload、`emfor check-config` | 啟動 plugin（`command` 與 `env_pass` 沒有改變時保留原來的），並要它**驗證**每個服務的 `check_options`。錯誤出現在 `services."<name>".check_options.<key>`，所以錯誤的選項在設定套用之前就會被發現。 |
| 每次檢查 | daemon 送出一個請求，內含 backend，以及比服務的 `timeout` 略短的逾時（`timeout` 減 10%，最多少 100 ms）。 |
| plugin 結束 | 它等待中的檢查是 `error`（`plugin exited`），並在 1 秒、然後 2 秒、4 秒、8 秒、16 秒後重新啟動，最多 30 秒。在那之前它的檢查都是 `error`（`plugin <name> unavailable`）。它執行滿 30 秒後，延遲重新從頭算起。 |
| 連續 3 個請求沒有回答 | 它會被砍掉並重新啟動（`plugin <name> stopped answering: 3 requests in a row got no answer`）。 |
| 它不再讀取輸入，且有 1024 個請求在等待 | 新的檢查是 `error`：`plugin <name> busy: it is not reading its requests`。 |
| 它寫到 standard error | 這些行以 `info` 記錄（`plugin <name>: <line>`），一次最多 100 行，之後每秒 10 行。 |
| 它停止或被停止 | 它的整個 process group 都會被砍掉，所以它啟動的程式不會殘留。 |

當掉的 plugin 絕不會把 backend 標成 down：它的檢查是 `error`，不算數。但一個 backend 的檢查只給出錯誤的時間超過 `fall × interval` 時，它會回到 `unknown`，所以一直當掉的 plugin 會讓它的 backend 變成 `unknown`，而不是 `healthy`。請留意 `emfor_plugin_up` 與 `emfor_plugin_restarts_total`（[第 12 章](12-operating.md#metrics)）。

## emfor-check 各檢查共用的選項

### TLS

大部分的檢查接受同樣的 TLS 鍵：

| 鍵 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `tls` | 布林 | `false` | 從第一個位元組就使用 TLS（LDAPS、SMTPS、rediss 等）。 |
| `tls_verify` | 布林 | `true` | `false` 接受任何憑證。需要開啟 TLS。 |
| `server_name` | 字串 | backend 的 IP | 憑證必須符合的名稱；也作為 SNI 送出。需要開啟 TLS。 |
| `ca_file` | 絕對路徑 | 系統的 CA | 要信任的 CA 憑證所組成的 PEM 檔。載入設定時檢查，檔案改變時重新讀取。需要開啟 TLS。 |

沒有 TLS 卻設定 `tls_verify`、`server_name` 或 `ca_file` 是錯誤（`requires tls = true`；ldap 與 smtp 是 `requires tls = true or starttls = true`）。

常見的 TLS reason：`tls: no answer to the TLS handshake (does the port speak
TLS?)`、`tls: connection closed during the TLS handshake (does the server have
a certificate?)`，以及作為 `error` 的 `tls: no system CA certificates found;
set ca_file`。

### 密碼

密碼絕不寫在設定檔裡。`password_file` 指定一個存放密碼的檔案，而且：

- 必須是絕對路徑；
- **每次**檢查都會重新讀取，所以改密碼不需要 reload；
- 去掉一個結尾的換行（`\n` 或 `\r\n`），之後必須不是空的，且是有效的 UTF-8；
- 檢查時讀不到它，結果是 `error`（`password_file: entity not found` 之類），不是 `down`。

讓這個檔案只有擁有者與 daemon 的群組能讀：

```sh
# install -d -m 0750 -o root -g emfor /etc/emfor/secrets
# install -m 0640 -o root -g emfor /dev/null /etc/emfor/secrets/pg.pass
# printf '%s\n' 'the-password' > /etc/emfor/secrets/pg.pass
```

`emfor check-config` 與 daemon 會對兩件事發出警告（只是警告，從不是錯誤）：

- 每個使用者都能讀、或其他人能修改的 `password_file`（或 `secret_file`、`tls_key_file`）：
  `password_file /etc/emfor/redis.pass (mode 0644) can be read by every user on this host; used by cache.svc.emfor. Limit it to its owner and the daemon's group, for example chmod 0640`；
- 不經 TLS 送出的密碼，適用於 `redis`、`ldap`（除非用 `starttls`）、`mysql`、`pgsql` 與 `amqp`：
  `service cache.svc.emfor: check_options has a password_file but tls is not true: the password is sent in clear text, and anyone who can see the traffic can read it`。

### 角色與複寫延遲

資料庫的檢查可以跟著 primary 走，讓 `balance = "failover"` 永遠給出目前的 primary：

| 檢查 | `role` 的值 | `max_lag` |
| --- | --- | --- |
| `redis` | `master`、`replica` | 搭配 `role = "replica"`：replica 可以落後的秒數（`master_last_io_seconds_ago`） |
| `mysql` | `primary`（`@@global.read_only` 為 0）、`replica`（兩個複寫執行緒都在執行） | 搭配 `role = "replica"`：`Seconds_Behind_Source` |
| `pgsql` | `primary`（不在 recovery 中）、`replica`（在 recovery 中且有 WAL receiver） | 搭配 `role = "replica"`：replay 延遲 |
| `mongodb` | `primary`（primary、standalone 或 mongos）、`secondary` | 無 |

`max_lag` 是 1 到 86400 的整數。讀不到延遲的 replica 是 `down`（`replication: lag unknown`）。

## 各項檢查

`emfor-check` 以檢查種類作為參數：
`emfor-check redis|mysql|pgsql|ldap|dns|radius|sip|diameter|smtp|amqp|mongodb|grpc|quic`。
每次檢查都開一條新連線，再把它關掉。有兩個例外：QUIC 每個位址族共用一個 UDP socket；`diameter` 在結果決定之後才在背景送出 Disconnect-Peer-Request，所以它的連線可能比檢查多存在不到一秒。下面的表格列出每個選項；TLS 鍵就是[上面](#tls)那些。

### redis

`PING` 回答 `PONG` 時為 `up`；設了密碼時要在 `AUTH` 之後，設了 `role` 時角色（與延遲）也要符合。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 無 | 需要 `password_file`。作為 AUTH 的使用者名稱送出（Redis 6 ACL）。 |
| `password_file` | 無 | 啟用 `AUTH`。 |
| `role` | 任何 | `master` 或 `replica`。replica 與 master 的連結必須是 up。 |
| `max_lag` | 無 | 1–86400；需要 `role = "replica"`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: ...`（伺服器回顯密碼時，密碼顯示為 `***`）、
`query: NOAUTH Authentication required.`、`role is replica, want master`、
`replication: link to master is sync`、`replication: 16s behind, max 15s`。

```toml
[plugins."redis"]
command = ["/usr/bin/emfor-check", "redis"]

[services."cache.svc.emfor"]
check = "redis"
balance = "failover"
backends = ["10.0.5.11:6379", "10.0.5.12:6379"]
check_options = { role = "master", tls = true, server_name = "cache.internal", password_file = "/etc/emfor/secrets/redis.pass" }
```

### mysql

登入成功且 `SELECT 1` 回傳 1（且角色符合）時為 `up`。已在 emfor 的測試套件中以 MariaDB 與 MySQL 8.0、8.4 測試。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 必填 | 不含 NUL。 |
| `password_file` | 無（沒有密碼） | |
| `role` | 任何 | `primary` 或 `replica`。 |
| `max_lag` | 無 | 1–86400；需要 `role = "replica"`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: ERROR 1045 (28000): Access denied ...`、
`tls: server does not support TLS`、`role is read-only, want primary`、
`replication: IO thread is Connecting: ...`、`replication: 31s behind, max 30s`。

### pgsql

登入成功且 `SELECT 1` 回傳 1（且角色符合）時為 `up`。連線使用 `application_name = emfor-check`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 必填 | 不含 NUL。 |
| `database` | 必填 | 不含 NUL。 |
| `password_file` | 無 | 密碼不含 NUL。 |
| `role` | 任何 | `primary` 或 `replica`。 |
| `max_lag` | 無 | 1–86400；需要 `role = "replica"`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | 有 TLS 時連線要求 TLS；沒有時 TLS 關閉。 |

reason：`auth: 28P01: password authentication failed for user "emfor"`、
`role is standby, want primary`、`role is primary, want replica`、
`replication: no WAL receiver`、`replication: 31s behind, max 30s`。

```toml
[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", tls = true, ca_file = "/etc/emfor/ca.pem", server_name = "db.internal", password_file = "/etc/emfor/secrets/pg.pass" }

[services."db-ro.svc.emfor"]
check = "pgsql"
backends = ["10.0.3.1:5432", "10.0.3.2:5432", "10.0.3.3:5432"]
check_options = { user = "emfor", database = "postgres", role = "replica", max_lag = 30, password_file = "/etc/emfor/secrets/pg.pass" }
```

### ldap

LDAPv3 simple bind 成功時為 `up`（沒有 `user` 時為匿名）。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 匿名 | bind DN；需要 `password_file`。 |
| `password_file` | 無 | 需要 `user`。 |
| `tls` | `false` | LDAPS。 |
| `starttls` | `false` | 在一般連線上使用 StartTLS；不能與 `tls` 一起用。 |
| `tls_verify`、`server_name`、`ca_file` | | 需要 `tls` 或 `starttls`。 |

reason：`auth: 49 invalidCredentials`、`auth: 8 strongerAuthRequired`、
`tls: StartTLS refused: 2 protocolError`、
`connect: connection closed by the server`。對 Active Directory 只在沒有憑證的情況下試過（上面那個 TLS reason）。

### dns

一次查詢得到 `expect_rcode` 中的回應碼（設了 `expect_answer` 時，還要有所查類型的記錄）時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `name` | 必填 | 一個網域名稱，或 `.`。 |
| `type` | `A` | `A`、`AAAA`、`CNAME`、`MX`、`NS`、`PTR`、`SOA`、`SRV`、`TXT`。 |
| `transport` | `udp` | `udp` 或 `tcp`。走 UDP 時，在逾時的一半會再送一次查詢。 |
| `recursion_desired` | `true` | RD 旗標。 |
| `expect_rcode` | `["NOERROR"]` | `NOERROR`、`FORMERR`、`SERVFAIL`、`NXDOMAIN`、`NOTIMP`、`REFUSED` 中的任何一個。 |
| `expect_answer` | `false` | 要求至少一筆 `type` 的記錄。 |

reason：`rcode: SERVFAIL`、`answer: no A records`；回答被截斷時加上
`(truncated; set transport = "tcp")`。

```toml
[plugins."dns"]
command = ["/usr/bin/emfor-check", "dns"]

[services."resolver.svc.emfor"]
check = "dns"
backends = ["10.0.0.53:53", "10.0.1.53:53"]
check_options = { name = "example.com", type = "A", expect_answer = true }
```

### radius

Status-Server（預設）或 Access-Request 得到正確的回答、且以共用密鑰簽署時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `secret_file` | 必填 | 共用密鑰，每次檢查都會讀取。 |
| `request` | `status-server` | 或 `access-request`。 |
| `application` | `auth` | `auth`（Access-Accept 為 up）或 `acct`（Accounting-Response 為 up；只能搭配 `status-server`）。 |
| `user` | 無 | 1–253 位元組；搭配 `access-request` 時必填，其他情況不允許。 |
| `password_file` | 無 | 最多 128 位元組；搭配 `access-request` 時必填，其他情況不允許。以 PAP 送出。 |
| `reject_is_up` | `false` | 搭配 `access-request`：Access-Reject 算作 up（伺服器有回答，只是拒絕了測試用的使用者）。 |
| `nas_identifier` | `emfor` | 1–253 位元組。 |
| `require_message_authenticator` | `false` | 沒有 Message-Authenticator 的回應是 down。 |

reason：`auth: Access-Reject: <Reply-Message>`、
`auth: response authenticator does not match (wrong secret?)`、
`query: unexpected response code 2`。

### sip

OPTIONS 請求得到 `expect_status` 中的最終狀態時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `transport` | `udp` | `udp` 或 `tcp`；`tls = true` 時必須是 `tcp`。 |
| `uri` | `sip:<ip>:<port>`（有 TLS 時為 `sips:`） | `sip:` 或 `sips:` URI，最多 256 位元組，不含空白、`<` 或 `>`。 |
| `from` | `sip:emfor@emfor.invalid` | 規則相同。 |
| `expect_status` | `["200-499"]` | `"NNN"` 或 `"lo-hi"` 形式的字串，代碼 200–699（不接受數字）。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`status: 503 Service Unavailable`、
`query: response does not match the request`。

### diameter

Capabilities-Exchange（CER/CEA）與 Device-Watchdog（DWR/DWA）都以 2xxx 的 Result-Code 成功時為 `up`。之後送出 Disconnect-Peer-Request。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `origin_host`、`origin_realm` | 必填 | DiameterIdentity：1–255 個 `A-Za-z0-9.-` 字元。 |
| `auth_application_ids`、`acct_application_ids` | `[]` | 整數清單；合計至少一個 application。 |
| `vendor_specific_application_ids` | `[]` | table，含 `vendor_id`，以及 `auth_application_id`、`acct_application_id` 其中之一。 |
| `supported_vendor_ids` | `[]` | 整數清單。 |
| `host_ip_address` | 本機位址 | 一個 IP 位址。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |
| `tls_cert_file`、`tls_key_file` | 無 | client 憑證（PEM）與它的私鑰，兩者一起設定；需要 `tls = true`。載入設定時檢查兩者是否相符。 |

reason：`cea: 3010 DIAMETER_UNKNOWN_PEER`、
`cea: 5010 DIAMETER_NO_COMMON_APPLICATION`、`dwa: 3004 DIAMETER_TOO_BUSY`、
`cea: connection closed by peer`。

### smtp

歡迎訊息是 220 且 `EHLO` 回答 250 時為 `up`；設了 `starttls` 時，STARTTLS 與第二次 `EHLO` 之後也要如此。不登入，也不寄送任何郵件。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `helo` | 本機位址的 literal | 主機名稱，1–255 個 `A-Za-z0-9.-` 字元。 |
| `tls` | `false` | Implicit TLS（port 465）。 |
| `starttls` | `false` | 不能與 `tls` 一起用。 |
| `tls_verify`、`server_name`、`ca_file` | | 需要 `tls` 或 `starttls`。 |

reason：`greeting: 554 5.3.2 no service`、`ehlo: 550 5.7.1 go away`、
`tls: STARTTLS not offered`。

### amqp

AMQP 0-9-1 登入（SASL PLAIN）成功且 vhost 開啟時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 必填 | 1–255 位元組，不含 NUL。 |
| `password_file` | 必填 | 不含 NUL；登入必須放得進 AMQP 4096 位元組的第一個 frame。 |
| `vhost` | `/` | 1–255 位元組。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: 403 ACCESS_REFUSED - ...`、`vhost: 530 NOT_ALLOWED - ...`、
`auth: server does not offer PLAIN`。

### mongodb

`hello` 有回答、SCRAM-SHA-256 登入成功（設了 `user` 時）且角色符合時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 不登入 | 1–255 位元組；需要 `password_file`。 |
| `password_file` | 無 | 需要 `user`；只能是可列印的 ASCII。 |
| `auth_source` | `admin` | 需要 `user`。1–64 個 `A-Za-z0-9_-` 字元。 |
| `role` | 任何 | `primary` 或 `secondary`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: AuthenticationFailed: Authentication failed.`、
`role is secondary, want primary`。沒有密碼警告：SCRAM 不會送出密碼。

### grpc

標準的健康服務（`grpc.health.v1.Health/Check`）回答 `SERVING` 時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `service` | `""`（整台伺服器） | 最多 256 位元組。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | 有 TLS 時要求 ALPN `h2`。 |

reason：`status: NOT_SERVING`、`grpc: 12 UNIMPLEMENTED`、
`tls: server did not select h2 (ALPN)`。

### quic（以及 HTTP/3）

QUIC 握手完成、憑證通過驗證，且 ALPN 是 `alpn` 中的一個時為 `up`；設了 `http3 = true` 時，`GET` 還必須得到 `expect_status` 中的狀態。QUIC 一律使用 TLS，所以沒有 `tls` 鍵。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `alpn` | `["h3"]` | 由 1–255 位元組的字串組成的非空清單。 |
| `tls_verify`、`server_name`、`ca_file` | | 同上，但不需要 `tls`。 |
| `http3` | `false` | 送出 HTTP/3 `GET`；`alpn` 中需要有 `"h3"`。 |
| `path` | `/` | 需要 `http3 = true`。 |
| `host_header` | 無 | 需要 `http3 = true`。 |
| `expect_status` | `["200-399"]` | 代碼 100–599 或範圍；需要 `http3 = true`。 |
| `expect_body` | 無 | 1–16384 位元組；需要 `http3 = true`。 |

reason：`tls: no application protocol (ALPN)`、`quic: closed by peer: 2`、
`unexpected status 503`、`expect_body: got "..."`。

```toml
[plugins."quic"]
command = ["/usr/bin/emfor-check", "quic"]

[services."edge.svc.emfor"]
check = "quic"
backends = ["10.0.4.11:443", "10.0.4.12:443"]
check_options = { server_name = "edge.internal", ca_file = "/etc/emfor/ca.pem", http3 = true, path = "/healthz" }
```

### 測試過什麼

`emfor-check` 的檢查在專案的測試套件中測試，其中一部分是對容器裡真正的伺服器測試。並非每種組合都跑過：`ldap` 檢查對 Active Directory 只在沒有憑證的情況下跑過，而關於不經 TLS 送出密碼的警告沒有對真正的伺服器跑過。[docs/deploy/plugins.md](../deploy/plugins.md) 有每種檢查的更多細節。

## 自己寫 plugin

plugin 是任何使用**協定版本 1** 的程式：每行一個 JSON 物件，請求從它的 standard input 進來，回答寫到它的 standard output。完整說明見 [docs/plugins/protocol-v1.md](../plugins/protocol-v1.md)。

| daemon 送出 | plugin 回答 |
| --- | --- |
| `{"type":"hello","protocol":1}` | 5 秒內回答 `{"type":"hello","protocol":1,"name":"...","version":"..."}` |
| `{"type":"validate","id":7,"options":{...}}` | `{"type":"validate","id":7,"errors":[{"key":"...","message":"..."}]}`（選項沒有問題時為空清單） |
| `{"type":"check","id":8,"service":"x.svc.emfor","target":{"ip":"10.0.0.1","port":22},"options":{...},"timeout_ms":900}` | `{"type":"check","id":8,"result":"up"}`，或帶 `"reason"` 的 `"down"`/`"error"` |

重要的規則：

- 回答每一個請求，順序不拘，以 `id` 對應。同時處理多個檢查：一次只回答一個的 plugin 會延遲其他每一個檢查。
- 每寫一行就 flush。一行最多 65536 位元組。
- 在 `timeout_ms` 內回答；較晚的回答會被忽略，檢查是 `error`。
- 忽略你不認得的欄位；之後的版本可能會增加。
- 診斷訊息寫到 standard error；它們會進到 daemon 的 log。
- 輸入結束時（daemon 要停止你），把正在執行的做完並結束。

一個完整的 Python plugin，檢查 TCP 連線是否成功：

```python
#!/usr/bin/env python3
"""emfor plugin (protocol 1): up when a TCP connection succeeds."""
import errno, json, socket, sys, threading

LOCAL = {errno.EMFILE, errno.ENFILE, errno.ENOBUFS, errno.ENOMEM, errno.EADDRNOTAVAIL}
lock = threading.Lock()

def send(msg):
    with lock:                                   # 一次寫出完整的一行
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()

def check(req):
    try:
        t = req["target"]
        socket.create_connection((t["ip"], t["port"]), timeout=req["timeout_ms"] / 1000).close()
        send({"type": "check", "id": req["id"], "result": "up"})
    except socket.timeout:
        send({"type": "check", "id": req["id"], "result": "down", "reason": "timeout"})
    except OSError as e:
        result = "error" if e.errno in LOCAL else "down"   # 這台主機本身的問題是 error
        send({"type": "check", "id": req["id"], "result": result, "reason": f"connect: {e.strerror}"})

for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    if msg["type"] == "hello":
        send({"type": "hello", "protocol": 1, "name": "tcpconnect", "version": "1.0"})
    elif msg["type"] == "validate":                # 這個 plugin 不接受任何選項
        send({"type": "validate", "id": msg["id"],
              "errors": [{"key": k, "message": "unknown key"} for k in msg["options"]]})
    elif msg["type"] == "check":
        threading.Thread(target=check, args=(msg,), daemon=True).start()
```

```toml
[plugins."tcpconnect"]
command = ["/usr/bin/python3", "-u", "/usr/local/libexec/emfor-tcpconnect.py"]

[services."ssh.svc.emfor"]
check = "tcpconnect"
backends = ["10.0.0.11:22", "10.0.0.12:22"]
```

這個 plugin 曾在 Debian 容器中以 emfor 0.7.0 執行，一個 port 有程式在聽、一個 port 是關閉的：

```console
# emfor status --service ssh.svc.emfor
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.9s ago, up 2.9s

ssh.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    2.7s ago  1.7ms
  127.0.0.1:8081  unhealthy  2.4s ago  -      connect: Connection refused
```

log 顯示 `plugin tcpconnect started (pid 47, tcpconnect 1.0)`，而
`check_options = { foo = 1 }` 會讓 `emfor check-config` 回報
`services."ssh.svc.emfor".check_options.foo: unknown key`。正式使用的 plugin 還應該能撐過它無法解析的一行。

### 用 Rust：emfor-plugin crate

`emfor-plugin` crate（在 `crates/emfor-plugin`）實作了這個協定：你寫 `validate` 與 `check`，其餘由 `emfor_plugin::run` 處理（並行、逾時，以及把檢查中的 panic 回答為 `error` `check panicked: ...`）。

```rust
use emfor_plugin::{CheckOutcome, CheckRequest, OptionError, Options, Plugin};
use tokio::net::TcpStream;

struct TcpConnect;

impl Plugin for TcpConnect {
    fn name(&self) -> &str { "tcpconnect" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn validate(&self, options: &Options) -> Vec<OptionError> {
        options.keys().map(|k| OptionError::new(k, "unknown key")).collect()
    }
    async fn check(&self, req: CheckRequest) -> CheckOutcome {
        match TcpStream::connect((req.ip, req.port)).await {
            Ok(_) => CheckOutcome::Up,
            Err(e) => CheckOutcome::Down(format!("connect: {e}")),
        }
    }
}

fn main() -> std::process::ExitCode {
    emfor_plugin::run(TcpConnect)
}
```

在 1.0 之前，這個 crate 的 API 跟著 emfor 的 minor 版本走，版本之間可能改變（[docs/compatibility.md](../compatibility.md)）。這個範例沒有為本手冊編譯過；`emfor-check` 本身就是建立在這個 crate 上。
