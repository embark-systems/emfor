# 7. Plugin 與 emfor-check

[手冊](README.md) · 上一章：[6. 健康檢查](06-health-checks.md) · 下一章：[8. 如何選出 backend](08-choosing-a-backend.md)

**plugin** 是一支長時間執行的程式：daemon 只啟動一次，之後把檢查請求送過去。所有用到這個 plugin 的服務都由同一個行程處理，而且它可以在兩次檢查之間保留狀態。emfor 內附一支 plugin 程式 `emfor-check`，裡面有十三種檢查；你也可以[自己寫](#自己寫-plugin)。

## 使用 plugin

先在 `[plugins]` 底下宣告 plugin，再把 plugin 名稱填進服務的 `check`：

```toml
[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }
```

- plugin 名稱由 1–63 個 `a-z`、`0-9`、`_`、`-` 字元組成，不能是 `tcp`、`http` 或 `exec`。名稱不必和參數一樣：名為 `pg-main` 的 plugin 也可以執行 `emfor-check pgsql`。
- `command` 是程式和參數，不經過 shell 執行。第一個元素要是可執行檔的絕對路徑。用套件安裝時，`emfor-check` 在 `/usr/bin/emfor-check`；用 FreeBSD port 安裝時在 `/usr/local/bin/emfor-check`；從原始碼建置的話，就在你安裝的位置（[第 4 章](04-installing.md#從原始碼建置)）。
- `env_pass` 列出除了 `PATH`、`LANG`、`TZ` 與 `LC_*` 以外，plugin 還會拿到的變數（[第 6 章](06-health-checks.md#檢查程式拿到的環境)）。
- 宣告了的 plugin 都會啟動，不管有沒有服務用到。
- port 一律取自 backend 位址；所有 plugin 都沒有 `port` 選項。

### daemon 如何處理 plugin

| 何時 | 發生什麼 |
| --- | --- |
| 啟動、reload、`emfor check-config` | 啟動 plugin（`command` 和 `env_pass` 都沒改的話，沿用原本的行程），再請它**驗證**每個服務的 `check_options`。錯誤會標在 `services."<name>".check_options.<key>`，所以選項寫錯，在設定套用前就會發現。 |
| 每次檢查 | daemon 送出一個請求，裡面有 backend，以及比服務的 `timeout` 稍短一點的逾時（`timeout` 減 10%，但最多只少 100 ms）。 |
| plugin 結束 | 還在等的檢查都算 `error`（`plugin exited`）。daemon 會在 1 秒後重新啟動 plugin，之後依序隔 2 秒、4 秒、8 秒、16 秒，最多 30 秒。重新啟動之前，這個 plugin 的檢查都是 `error`（`plugin <name> unavailable`）。plugin 連續跑滿 30 秒後，延遲就重新從頭算。 |
| 連續 3 個請求沒有回答 | daemon 會砍掉 plugin 再重新啟動（`plugin <name> stopped answering: 3 requests in a row got no answer`）。 |
| plugin 不再讀輸入，而且有 1024 個請求在排隊 | 新的檢查都是 `error`：`plugin <name> busy: it is not reading its requests`。 |
| plugin 寫到 standard error | 這些行會以 `info` 記進 log（`plugin <name>: <line>`），一次最多 100 行，之後每秒最多 10 行。 |
| plugin 自己結束或被停止 | daemon 會砍掉它整個 process group，它啟動的程式不會留下來。 |

plugin 當掉，絕對不會讓 backend 被標成 down：它的檢查結果是 `error`，不列入計算。不過，如果一個 backend 的檢查一直只有錯誤，超過 `fall × interval` 就會回到 `unknown`。所以 plugin 一直當掉時，它的 backend 會變成 `unknown`，而不是 `healthy`。請留意 `emfor_plugin_up` 與 `emfor_plugin_restarts_total`（[第 12 章](12-operating.md#metrics)）。

## emfor-check 各檢查共用的選項

### TLS

大部分的檢查都用同一組 TLS 鍵：

| 鍵 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `tls` | 布林 | `false` | 從第一個位元組就走 TLS（LDAPS、SMTPS、rediss 等）。 |
| `tls_verify` | 布林 | `true` | 設成 `false` 會接受任何憑證。要開啟 TLS 才能用。 |
| `server_name` | 字串 | backend 的 IP | 憑證必須符合的名稱，也會當作 SNI 送出。要開啟 TLS 才能用。 |
| `ca_file` | 絕對路徑 | 系統的 CA | 一個 PEM 檔，裝著要信任的 CA 憑證。載入設定時會檢查，檔案有變動就重新讀取。要開啟 TLS 才能用。 |

沒開 TLS 卻設了 `tls_verify`、`server_name` 或 `ca_file`，會被當成錯誤（`requires tls = true`；ldap 和 smtp 是 `requires tls = true or starttls = true`）。

常見的 TLS reason 有：`tls: no answer to the TLS handshake (does the port speak
TLS?)`、`tls: connection closed during the TLS handshake (does the server have
a certificate?)`，還有歸為 `error` 的 `tls: no system CA certificates found;
set ca_file`。

### 密碼

密碼一律不寫在設定檔裡。`password_file` 指向一個存放密碼的檔案，規則如下：

- 必須是絕對路徑；
- **每次**檢查都會重新讀取，所以改密碼不用 reload；
- 讀取時會去掉結尾的一個換行（`\n` 或 `\r\n`），去掉後不能是空的，而且要是有效的 UTF-8；
- 檢查時讀不到這個檔案，結果是 `error`（例如 `password_file: entity not found`），不是 `down`。

請讓這個檔案只有擁有者和 daemon 的群組能讀：

```sh
# install -d -m 0750 -o root -g emfor /etc/emfor/secrets
# install -m 0640 -o root -g emfor /dev/null /etc/emfor/secrets/pg.pass
# printf '%s\n' 'the-password' > /etc/emfor/secrets/pg.pass
```

遇到下面兩種情況，`emfor check-config` 和 daemon 會發出警告（只會是警告，不會當成錯誤）：

- `password_file`（或 `secret_file`、`tls_key_file`）每個使用者都能讀，或其他人能修改：
  `password_file /etc/emfor/redis.pass (mode 0644) can be read by every user on this host; used by cache.svc.emfor. Limit it to its owner and the daemon's group, for example chmod 0640`；
- 密碼沒有經過 TLS 就送出，適用於 `redis`、`ldap`（用 `starttls` 時除外）、`mysql`、`pgsql` 與 `amqp`：
  `service cache.svc.emfor: check_options has a password_file but tls is not true: the password is sent in clear text, and anyone who can see the traffic can read it`。

### 角色與複寫延遲

資料庫的檢查可以跟著 primary 走，讓 `balance = "failover"` 一定選到目前的 primary：

| 檢查 | `role` 的值 | `max_lag` |
| --- | --- | --- |
| `redis` | `master`、`replica` | 搭配 `role = "replica"`：replica 可以落後的秒數（`master_last_io_seconds_ago`） |
| `mysql` | `primary`（`@@global.read_only` 為 0）、`replica`（兩個複寫執行緒都在跑） | 搭配 `role = "replica"`：`Seconds_Behind_Source` |
| `pgsql` | `primary`（不在 recovery 中）、`replica`（在 recovery 中且有 WAL receiver） | 搭配 `role = "replica"`：replay 延遲 |
| `mongodb` | `primary`（primary、standalone 或 mongos）、`secondary` | 無 |

`max_lag` 是 1 到 86400 的整數。如果讀不到 replica 的延遲，結果是 `down`（`replication: lag unknown`）。

## 各項檢查

`emfor-check` 用參數指定檢查種類：
`emfor-check redis|mysql|pgsql|ldap|dns|radius|sip|diameter|smtp|amqp|mongodb|grpc|quic`。
每次檢查都會開一條新連線，用完就關。例外有兩個：QUIC 每個位址族共用一個 UDP socket；`diameter` 則是等結果確定後，才在背景送出 Disconnect-Peer-Request，所以連線可能比檢查多留不到一秒。下面的表格列出所有選項；TLS 鍵就是[上面](#tls)那幾個。

### redis

`PING` 回答 `PONG` 時為 `up`。有設密碼的話，要在 `AUTH` 之後回答；有設 `role` 的話，角色（和延遲）也要符合。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 無 | 需要 `password_file`。會當作 AUTH 的使用者名稱送出（Redis 6 ACL）。 |
| `password_file` | 無 | 設了就會用 `AUTH`。 |
| `role` | 任何 | `master` 或 `replica`。replica 連到 master 的連結必須是 up。 |
| `max_lag` | 無 | 1–86400；需要 `role = "replica"`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: ...`（伺服器如果把密碼原樣回傳，密碼會顯示成 `***`）、
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

登入成功、`SELECT 1` 回傳 1（而且角色符合）時為 `up`。emfor 的測試套件測過 MariaDB 以及 MySQL 8.0 和 8.4。

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

登入成功、`SELECT 1` 回傳 1（而且角色符合）時為 `up`。連線時會帶 `application_name = emfor-check`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 必填 | 不含 NUL。 |
| `database` | 必填 | 不含 NUL。 |
| `password_file` | 無 | 密碼不含 NUL。 |
| `role` | 任何 | `primary` 或 `replica`。 |
| `max_lag` | 無 | 1–86400；需要 `role = "replica"`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | 開了 TLS，連線就要求 TLS；沒開就不用 TLS。 |

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

LDAPv3 simple bind 成功時為 `up`（沒設 `user` 就用匿名 bind）。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 匿名 | bind DN；需要 `password_file`。 |
| `password_file` | 無 | 需要 `user`。 |
| `tls` | `false` | LDAPS。 |
| `starttls` | `false` | 在一般連線上用 StartTLS；不能和 `tls` 一起用。 |
| `tls_verify`、`server_name`、`ca_file` | | 需要 `tls` 或 `starttls`。 |

reason：`auth: 49 invalidCredentials`、`auth: 8 strongerAuthRequired`、
`tls: StartTLS refused: 2 protocolError`、
`connect: connection closed by the server`。搭配 Active Directory 只在沒有憑證的狀況下試過（也就是上面那個 TLS reason）。

### dns

查詢一次，回應碼在 `expect_rcode` 裡（設了 `expect_answer` 時，還要有查詢類型的記錄）時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `name` | 必填 | 一個網域名稱，或 `.`。 |
| `type` | `A` | `A`、`AAAA`、`CNAME`、`MX`、`NS`、`PTR`、`SOA`、`SRV`、`TXT`。 |
| `transport` | `udp` | `udp` 或 `tcp`。走 UDP 時，過了逾時的一半會再送一次查詢。 |
| `recursion_desired` | `true` | RD 旗標。 |
| `expect_rcode` | `["NOERROR"]` | `NOERROR`、`FORMERR`、`SERVFAIL`、`NXDOMAIN`、`NOTIMP`、`REFUSED` 中的任何值。 |
| `expect_answer` | `false` | 至少要有一筆 `type` 的記錄。 |

reason：`rcode: SERVFAIL`、`answer: no A records`；回答被截斷時會加上
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

Status-Server（預設）或 Access-Request 收到正確的回答，而且回答以共用密鑰簽署時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `secret_file` | 必填 | 共用密鑰，每次檢查都會重新讀取。 |
| `request` | `status-server` | 或 `access-request`。 |
| `application` | `auth` | `auth`（Access-Accept 為 up）或 `acct`（Accounting-Response 為 up；只能搭配 `status-server`）。 |
| `user` | 無 | 1–253 位元組；搭配 `access-request` 時必填，其他情況不允許。 |
| `password_file` | 無 | 最多 128 位元組；搭配 `access-request` 時必填，其他情況不允許。以 PAP 送出。 |
| `reject_is_up` | `false` | 搭配 `access-request`：收到 Access-Reject 也算 up（伺服器有回答，只是拒絕了測試用的使用者）。 |
| `nas_identifier` | `emfor` | 1–253 位元組。 |
| `require_message_authenticator` | `false` | 回應裡沒有 Message-Authenticator 就算 down。 |

reason：`auth: Access-Reject: <Reply-Message>`、
`auth: response authenticator does not match (wrong secret?)`、
`query: unexpected response code 2`。

### sip

OPTIONS 請求收到的最終狀態在 `expect_status` 裡時為 `up`。

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

Capabilities-Exchange（CER/CEA）和 Device-Watchdog（DWR/DWA）都成功、Result-Code 都是 2xxx 時為 `up`。之後會再送出 Disconnect-Peer-Request。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `origin_host`、`origin_realm` | 必填 | DiameterIdentity：1–255 個 `A-Za-z0-9.-` 字元。 |
| `auth_application_ids`、`acct_application_ids` | `[]` | 整數清單；合計至少一個 application。 |
| `vendor_specific_application_ids` | `[]` | table，含 `vendor_id`，以及 `auth_application_id`、`acct_application_id` 其中之一。 |
| `supported_vendor_ids` | `[]` | 整數清單。 |
| `host_ip_address` | 本機位址 | 一個 IP 位址。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |
| `tls_cert_file`、`tls_key_file` | 無 | client 憑證（PEM）和對應的私鑰，兩個要一起設；需要 `tls = true`。載入設定時會檢查兩者是否相符。 |

reason：`cea: 3010 DIAMETER_UNKNOWN_PEER`、
`cea: 5010 DIAMETER_NO_COMMON_APPLICATION`、`dwa: 3004 DIAMETER_TOO_BUSY`、
`cea: connection closed by peer`。

### smtp

歡迎訊息是 220、`EHLO` 回答 250 時為 `up`；設了 `starttls` 的話，STARTTLS 之後第二次 `EHLO` 也要這樣。檢查不會登入，也不會寄出任何郵件。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `helo` | 本機位址的 literal | 主機名稱，1–255 個 `A-Za-z0-9.-` 字元。 |
| `tls` | `false` | Implicit TLS（port 465）。 |
| `starttls` | `false` | 不能和 `tls` 一起用。 |
| `tls_verify`、`server_name`、`ca_file` | | 需要 `tls` 或 `starttls`。 |

reason：`greeting: 554 5.3.2 no service`、`ehlo: 550 5.7.1 go away`、
`tls: STARTTLS not offered`。

### amqp

AMQP 0-9-1 登入（SASL PLAIN）成功且 vhost 開啟時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 必填 | 1–255 位元組，不含 NUL。 |
| `password_file` | 必填 | 不含 NUL；登入資料必須塞得進 AMQP 4096 位元組的第一個 frame。 |
| `vhost` | `/` | 1–255 位元組。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: 403 ACCESS_REFUSED - ...`、`vhost: 530 NOT_ALLOWED - ...`、
`auth: server does not offer PLAIN`。

### mongodb

`hello` 有回答、SCRAM-SHA-256 登入成功（有設 `user` 時），而且角色符合時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `user` | 不登入 | 1–255 位元組；需要 `password_file`。 |
| `password_file` | 無 | 需要 `user`；只能是可列印的 ASCII。 |
| `auth_source` | `admin` | 需要 `user`。1–64 個 `A-Za-z0-9_-` 字元。 |
| `role` | 任何 | `primary` 或 `secondary`。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | |

reason：`auth: AuthenticationFailed: Authentication failed.`、
`role is secondary, want primary`。這個檢查不會出現密碼警告，因為 SCRAM 不會送出密碼。

### grpc

標準的健康服務（`grpc.health.v1.Health/Check`）回答 `SERVING` 時為 `up`。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `service` | `""`（整台伺服器） | 最多 256 位元組。 |
| `tls`、`tls_verify`、`server_name`、`ca_file` | | 開了 TLS 時，ALPN 必須是 `h2`。 |

reason：`status: NOT_SERVING`、`grpc: 12 UNIMPLEMENTED`、
`tls: server did not select h2 (ALPN)`。

### quic（以及 HTTP/3）

QUIC 握手完成、憑證通過驗證，而且 ALPN 是 `alpn` 裡的其中一個時為 `up`；設了 `http3 = true` 的話，`GET` 拿到的狀態也必須在 `expect_status` 裡。QUIC 一定走 TLS，所以沒有 `tls` 鍵。

| 選項 | 預設值 | 規則 |
| --- | --- | --- |
| `alpn` | `["h3"]` | 不能是空清單，每個字串 1–255 位元組。 |
| `tls_verify`、`server_name`、`ca_file` | | 同上，但不需要 `tls`。 |
| `http3` | `false` | 送出 HTTP/3 `GET`；`alpn` 裡要有 `"h3"`。 |
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

`emfor-check` 的檢查都在專案的測試套件裡測過，其中一部分是拿容器裡真正的伺服器來測。不是每種組合都跑過：`ldap` 檢查搭配 Active Directory 只在沒有憑證的狀況下跑過；不經 TLS 送出密碼的警告，也沒有拿真正的伺服器跑過。每種檢查的細節請看 [docs/deploy/plugins.md](../deploy/plugins.md)。

## 自己寫 plugin

任何程式只要使用**協定版本 1**，就是 plugin：每行一個 JSON 物件，請求從 standard input 進來，回答寫到 standard output。完整說明請看 [docs/plugins/protocol-v1.md](../plugins/protocol-v1.md)。

| daemon 送出 | plugin 回答 |
| --- | --- |
| `{"type":"hello","protocol":1}` | 5 秒內回答 `{"type":"hello","protocol":1,"name":"...","version":"..."}` |
| `{"type":"validate","id":7,"options":{...}}` | `{"type":"validate","id":7,"errors":[{"key":"...","message":"..."}]}`（選項沒有問題時為空清單） |
| `{"type":"check","id":8,"service":"x.svc.emfor","target":{"ip":"10.0.0.1","port":22},"options":{...},"timeout_ms":900}` | `{"type":"check","id":8,"result":"up"}`，或帶 `"reason"` 的 `"down"`/`"error"` |

要注意的規則：

- 每個請求都要回答，順序不限，用 `id` 對應。檢查要並行處理：一次只回答一個的 plugin，會拖慢其他所有檢查。
- 每寫一行就 flush。一行最多 65536 位元組。
- 要在 `timeout_ms` 內回答；太晚的回答會被忽略，檢查算 `error`。
- 不認得的欄位就忽略；之後的版本可能會加新欄位。
- 診斷訊息寫到 standard error，會進到 daemon 的 log。
- 讀到輸入結束（表示 daemon 要停掉你），就把手上的檢查做完再結束。

下面是一個完整的 Python plugin，檢查 TCP 連線能不能建立：

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

這個 plugin 在 Debian 容器裡用 emfor 0.7.0 跑過，一個 port 有程式在聽，另一個 port 是關的：

```console
# emfor status --service ssh.svc.emfor
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.9s ago, up 2.9s

ssh.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    2.7s ago  1.7ms
  127.0.0.1:8081  unhealthy  2.4s ago  -      connect: Connection refused
```

log 裡會看到 `plugin tcpconnect started (pid 47, tcpconnect 1.0)`；如果寫了
`check_options = { foo = 1 }`，`emfor check-config` 會回報
`services."ssh.svc.emfor".check_options.foo: unknown key`。正式環境用的 plugin，還應該在讀到解析不了的一行時不會掛掉。

### 用 Rust：emfor-plugin crate

`emfor-plugin` crate（在 `crates/emfor-plugin`）實作了這個協定：你只要寫 `validate` 和 `check`，其他都交給 `emfor_plugin::run`（並行、逾時，還有檢查中發生 panic 時回答 `error` `check panicked: ...`）。

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

在 1.0 之前，這個 crate 的 API 跟著 emfor 的 minor 版本走，版本之間可能會變（[docs/compatibility.md](../compatibility.md)）。這個範例沒有為了這份手冊編譯過；`emfor-check` 本身就是用這個 crate 寫的。
