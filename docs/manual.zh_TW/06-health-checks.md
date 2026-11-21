# 6. 健康檢查：tcp、http 與 exec

[手冊](README.md) · 上一章：[5. 設定 emfor](05-configuring.md) · 下一章：[7. Plugin](07-plugins.md)

服務的 `check` 決定如何測試每個 backend，它的 `check_options` table 則設定這項測試。emfor 有三種內建檢查，其他一切都交給 plugin。

| `check` | 測試什麼 | 適合 |
| --- | --- | --- |
| `tcp` | 一條 TCP 連線；可選擇加上 TLS 與 send/expect 交換 | 任何 TCP 服務；以歡迎訊息開場的協定，例如 SMTP 或 IMAP |
| `http` | 一個 `GET` 請求與它的狀態碼（可選擇再加上內容） | Web 服務與 health endpoint |
| `exec` | 每次檢查執行一次的程式 | 既有的 script 與 Nagios 外掛、不頻繁的檢查 |
| plugin 名稱 | 長時間執行的 plugin 程式 | 資料庫與各種協定（[第 7 章](07-plugins.md)） |

每次檢查都以 `up`、`down` 或 `error` 結束（[第 3 章](03-concepts.md#檢查結果)）。在服務的 `timeout` 內沒有完成的檢查，結果是 `down`，reason 為 `timeout`；plugin 的檢查例外，結果是 `error`（`plugin did not answer within timeout`）：此時遲到的是 plugin，不是 backend。

## tcp

沒有選項時，backend 接受 TCP 連線即為 `up`；emfor 隨即關閉連線。

```toml
[services."redis-raw.svc.emfor"]
check = "tcp"
backends = ["10.0.5.11:6379", "10.0.5.12:6379"]
```

連線被拒絕或無法到達為 `down`，本機沒有通往 backend 的路由、或本機自己的防火牆擋下連線時也是：這台主機上的應用程式同樣連不到它。只有本機資源不足（開啟的檔案、緩衝區、記憶體、本機 port）才是 `error`。

| 選項 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `send` | 字串 | 無 | 連線後寫出一次的位元組。TOML 跳脫字元如 `\r\n` 會以對應的位元組送出。需要搭配 `expect`。 |
| `expect` | 字串 | 無 | backend 送來的資料中必須出現的位元組，區分大小寫（最多讀 16384 位元組）。不可為空，最長 16384 位元組。 |
| `tls` | 布林 | `false` | 連線後立即進行 TLS handshake。絕不退回明文。 |
| `tls_verify` | 布林 | `true` | `false` 接受任何憑證（連線仍然加密）。需要 `tls = true`。 |
| `server_name` | 字串 | backend 的 IP | 憑證必須符合的名稱（或 IP）；是名稱時也作為 SNI 送出。需要 `tls = true`。 |
| `ca_file` | 絕對路徑 | 系統 CA | 改為信任這個 PEM 檔中的 CA 憑證。載入設定時，檔案中必須至少有一張可以當作 CA 使用的憑證。需要 `tls = true`。 |

範例：

```toml
[services."smtp.svc.emfor"]
check = "tcp"
backends = ["10.0.6.11:25", "10.0.6.12:25"]
check_options = { expect = "220" }            # 歡迎訊息

[services."memcache.svc.emfor"]
check = "tcp"
backends = ["10.0.7.11:11211"]
check_options = { send = "stats\r\n", expect = "STAT pid" }

[services."imap.svc.emfor"]
check = "tcp"
backends = ["10.0.8.11:993"]
check_options = { tls = true, server_name = "imap.internal", expect = "* OK" }
```

常見的 reason：

| Reason | 意義 |
| --- | --- |
| `Connection refused (os error 111)` | 那個 port 沒有東西在 listen（連線錯誤本身，沒有前綴）。 |
| `timeout` | 在 `timeout` 內沒有建立連線，或沒有收到符合的回答。 |
| `expect: got "421 too busy\r\n"` | 回答中沒有 `expect`；後面是收到的前 64 位元組，已跳脫。 |
| `send: ...`、`expect: ...` | 傳送或讀取時發生 I/O 錯誤。 |
| `tls: no answer to the TLS handshake (does the port speak TLS?)` | 這個 port 不使用 TLS。 |
| `tls: connection closed during the TLS handshake (does the server have a certificate?)` | 伺服器直接關閉連線而不回答，例如 Active Directory 沒有憑證時的 636 port。 |
| `tls: ...` | 其他 TLS 失敗，例如憑證不符。 |

## http

每次檢查對 backend 的 IP 與 port 送一個 `GET` 請求。不會跟隨重新導向，也不使用 proxy。

```toml
[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]

[services."api.svc.emfor".check_options]
path = "/healthz"
host_header = "api.internal"
expect_status = ["200-299"]
expect_body = '"status":"UP"'
```

| 選項 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `scheme` | `"http"` 或 `"https"` | `"http"` | 協定。 |
| `path` | 以 `/` 開頭的字串 | `"/"` | 請求路徑，需要時可以帶 query string。`#`、`\` 與控制字元會被拒絕；空白或其他特殊字元會以百分比編碼送出。 |
| `host_header` | 字串 | 無 | `Host` header：主機名稱或 IP 位址，可加 `:port`，IPv6 要加中括號（`api.internal`、`api.internal:8443`、`[2001:db8::1]`）。連線仍然連到 backend；用 `https` 時，主機名稱也是 TLS 的 server name。 |
| `expect_status` | 非空清單 | `["200-399"]` | 視為 `up` 的狀態碼：單一狀態碼（`200` 或 `"200"`）或含頭尾的範圍（`"200-299"`），都必須在 100–599 之間。 |
| `expect_body` | 字串 | 無 | 回應內容中必須出現的位元組，區分大小寫。最多讀 65536 位元組的內容。不可為空，最長 16384 位元組。 |
| `tls_verify` | 布林 | `true` | 用於 `https`：`false` 接受任何憑證。 |

結果看起來的樣子：

| Reason | 意義 |
| --- | --- |
| `unexpected status 503` | 狀態碼不在 `expect_status` 中。不會讀內容。 |
| `expect_body: got "<first 64 bytes>"` | 狀態碼符合，但內容中沒有那段文字。 |
| `body: ...` | 無法讀取內容。 |
| `error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)` | 連線失敗。reason 從不包含請求 URL，所以 `path` 裡的 token 不會洩漏到 log。 |
| `timeout` | 在 `timeout` 內沒有收到完整的回答。 |
| `cannot build http client: ...; https checks need the system CA certificates unless tls_verify = false`（`error`） | 在沒有 CA 憑證的主機上（Debian 與 RHEL 上是 `ca-certificates`）執行驗證開著的 `https` 檢查。從 0.7.0 起，單純的 `http` 不需要 CA 憑證。 |

不論狀態好壞都回 200、把狀態寫在內容裡的健康端點，請用 `expect_body`。此時 `balance = "fastest"` 用的延遲包含讀內容的時間。

## exec

每次檢查執行一次程式，不經過 shell，工作目錄為 `/`，沒有標準輸入。backend 透過環境變數傳入。

```toml
[services."report.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/check_replication.sh", "--max-lag", "60"] }
```

對應的程式：

```sh
#!/bin/sh
# /usr/local/libexec/check_replication.sh：emfor 會設定 EMFOR_IP、EMFOR_PORT、EMFOR_SERVICE。
lag=$(psql -h "$EMFOR_IP" -p "$EMFOR_PORT" -U monitor -Atc \
      "select coalesce(extract(epoch from now() - pg_last_xact_replay_timestamp()), 0)::int" postgres) \
  || { echo "cannot query $EMFOR_IP"; exit 2; }
[ "$lag" -le "$2" ] || { echo "replication ${lag}s behind"; exit 1; }
exit 0
```

（這個 script 只是範例；沒有對真正的 PostgreSQL 執行過。）

| 程式 | 結果 |
| --- | --- |
| 以 0 結束 | `up` |
| 以 1 或 2 結束 | `down`；reason 為它標準輸出的第一行（截到 200 個字元），什麼都沒印時為 `exit code N` |
| 以其他代碼結束 | `error`，reason 為 `exit code N` 或 `exit code N: <first line>` |
| 被 signal 終止 | `error`（`killed by signal N`） |
| 無法啟動 | `error`（`cannot run <program>: ...`） |
| 到 `timeout` 仍在執行 | `down`（`timeout`）；程式與它啟動的每個行程都會被終止 |

這是 Nagios 的慣例，只是 **1（WARNING）算 down**，3（UNKNOWN）算 error。標準錯誤會被丟棄。

| 選項 | 型別 | 意義 |
| --- | --- | --- |
| `command` | 字串清單，必填 | 程式與它的參數。第一個元素必須是可執行檔的絕對路徑；任何元素都不能含 NUL。 |
| `env_pass` | 名稱清單 | 程式另外要拿到的 daemon 環境變數（[見下](#檢查程式拿到的環境)）。 |

每個 backend 的每次檢查都啟動一個行程，成本不低。需要頻繁檢查很多 backend 時，請改寫一個 [plugin](07-plugins.md#自己寫-plugin)。要使用既有的、命令列需要 `-H` 與 `-p` 的 Nagios 外掛，見[第 14 章](14-integrations.md#把-nagios-外掛當成檢查)的包裝 script。

## 檢查程式拿到的環境

從 0.7.0 起，`exec` 程式與 plugin **拿不到 daemon 的環境**。它們只拿到：

- `PATH`、`LANG`、`TZ` 與所有 `LC_*` 變數，值與 daemon 的相同；
- `exec` 另外拿到：`EMFOR_IP`、`EMFOR_PORT` 與 `EMFOR_SERVICE`；
- `env_pass` 中列出的變數。

所以 daemon 環境中的祕密（例如 unit 檔設定的）不會到檢查程式手上。需要其他變數（例如 `HOME` 或 `https_proxy`）的程式，在你列出它的名稱後就會拿到：

```toml
[services."report2.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432"]
check_options = { command = ["/usr/local/libexec/check_replication.sh", "--max-lag", "60"], env_pass = ["HOME", "PGSSLROOTCERT"] }

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]
env_pass = ["https_proxy"]
```

- `env_pass` 列的是**名稱**，絕不是值。daemon 沒有的名稱會略過，不是錯誤。要給程式一個 daemon 環境中沒有的值，在 unit 檔中設定它（`Environment=`），再列出它的名稱。
- 名稱由字母、數字與 `_` 組成，不能以數字開頭，而且只能出現一次。對 `exec` 而言，名稱不能是 `EMFOR_IP`、`EMFOR_PORT` 或 `EMFOR_SERVICE`（`env_pass: EMFOR_IP is set by emfor`）。
- `emfor-check` 在沒有 `ca_file` 的 TLS 檢查中，讀取 `SSL_CERT_FILE` 與 `SSL_CERT_DIR` 來找系統 CA。如果你依賴它們，把它們加進該 plugin 的 `env_pass`，或設定 `ca_file`。
- 改變 plugin 的 `env_pass` 會在 reload 時重新啟動該 plugin。
- 在聯防群組中，`env_pass` 屬於成員之間比對的檢查定義：同一個服務在各主機上必須列出相同的名稱。
- 程式需要卻拿不到的變數，`emfor check-config` 不會發出警告。從 0.6 升級之前，先找出讀取 `HOME`、proxy 變數或 `LD_LIBRARY_PATH` 的程式（[第 15 章](15-upgrading.md)）。

這在 container 中檢查過：設定 `env_pass = ["HOME"]`，並以 `HOME=/root MY_SECRET=hunter2` 啟動 daemon，檢查程式看到 `HOME=/root`，沒有 `MY_SECRET`。

## 排程

每個 backend 依自己的時程檢查，而且同一個 backend 不會同時有兩個檢查：

- 啟動或 reload 後的第一次檢查，在 `min(interval, 200ms)` 內的隨機時刻開始。
- backend 處於 `unknown` 且檢查結果為 `up` 或 `down` 時，下一次檢查在上一次結束後 `min(interval, 200ms)` 開始，最多 `rise + fall` 次。用預設值時，約半秒內每個 backend 的狀態都已確定。結果一直變來變去、兩個計數都到不了門檻的 backend 會維持 `unknown`，之後改為每 `interval` 檢查一次。
- 其他情況下，下一次檢查在上一次**結束**後 `interval` 開始，並隨機前後偏移最多 `jitter`（預設 10%）。因此慢的 backend 不會比每 `interval` 一次更頻繁地被檢查。
- 在聯防群組中，不是某個 backend 負責主機的成員，較少檢查它（[第 11 章](11-peer-groups.md#投票如何運作)）。

## rise、fall 與 error

- `up`：成功計數加一，失敗計數歸零；到達 `rise` 時 backend 變成 `healthy`。
- `down`：失敗計數加一，成功計數歸零；到達 `fall` 時 backend 變成 `unhealthy`。
- `error`：什麼都不變。backend 只收到 error 的時間超過 `fall × interval` 時，回到 `unknown`。

發現故障大約需要 `fall × interval`（預設值下 6 秒）；發現恢復大約需要 `rise × interval`（4 秒）。每次狀態變化都以 `info` 記錄，包含服務、backend、舊狀態與新狀態，以及 reason：

```
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason="..."
```

## reason 會被處理成安全的形式

儲存 reason 之前，emfor 會把它跳脫：tab、CR 與 LF 變成 `\t`、`\r` 與 `\n`；其他控制字元變成 `\u{1b}` 之類的形式；會隱藏或改變文字順序的不可見格式字元也一樣（雙向控制字元如 U+202E、零寬字元、軟連字號、tag 字元）。超過 512 個字元的 reason 會被截斷並以 `…` 結尾。因此 backend 無法讓 `emfor status` 或 log 行顯示與實際發生的不同的內容。

## 延遲

emfor 為每個 backend 量測每次 `up` 檢查花了多久，並保存一個平滑後的值（每個新樣本占 5%）。`emfor status` 與 `/metrics` 會顯示它，`balance = "fastest"` 會用到它。依賴這個值時，請選擇接近實際流量的檢查。

## 選擇 interval、timeout、rise 與 fall

| 目標 | 設定 |
| --- | --- |
| 預設值：大約 6 秒發現故障 | `interval = "2s"`、`timeout = "1s"`、`rise = 2`、`fall = 3` |
| 在快速網路上更快發現 | `interval = "500ms"`、`timeout = "300ms"`、`fall = 3`（約 1.5 秒） |
| 成本高的檢查（script、查詢） | `interval = "10s"` 或更長，`timeout` 遠低於它 |
| 偶爾會變慢的 backend | 調高 `timeout`（最多到 `interval`）與 `fall` |
| 狀態反覆變化的 backend（[第 16 章](16-troubleshooting.md#backend-反覆變化flapping)） | 調高 `rise` 與 `fall`；拿 `timeout` 和正常的回應時間比較 |

限制：`interval` 從 `100ms` 到 `24h`；`timeout` 至少 `10ms`，最多為 `interval`；`rise` 與 `fall` 至少 1；`jitter` 從 `0%` 到 `50%`。
