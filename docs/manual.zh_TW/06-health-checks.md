# 6. 健康檢查：tcp、http 與 exec

[手冊](README.md) · 上一章：[5. 設定 emfor](05-configuring.md) · 下一章：[7. Plugin](07-plugins.md)

服務的 `check` 決定怎麼測試每個 backend，`check_options` table 則用來設定這項測試。emfor 內建三種檢查，其他的都交給 plugin。

| `check` | 測試什麼 | 適合 |
| --- | --- | --- |
| `tcp` | 建一條 TCP 連線；可以再加上 TLS 與 send/expect 交換 | 任何 TCP 服務；一連上就送歡迎訊息的協定，例如 SMTP 或 IMAP |
| `http` | 送一個 `GET` 請求，看狀態碼（也可以再看內容） | Web 服務與 health endpoint |
| `exec` | 每次檢查都執行一次的程式 | 現有的 script 與 Nagios 外掛、不常跑的檢查 |
| plugin 名稱 | 長時間執行的 plugin 程式 | 資料庫與各種協定（[第 7 章](07-plugins.md)） |

每次檢查都以 `up`、`down` 或 `error` 結束（[第 3 章](03-concepts.md#檢查結果)）。檢查在服務的 `timeout` 內沒做完，結果就是 `down`，reason 為 `timeout`。plugin 的檢查例外，結果是 `error`（`plugin did not answer within timeout`），因為這時遲到的是 plugin，不是 backend。

## tcp

不加選項時，backend 只要接受 TCP 連線就算 `up`；emfor 會馬上關掉連線。

```toml
[services."redis-raw.svc.emfor"]
check = "tcp"
backends = ["10.0.5.11:6379", "10.0.5.12:6379"]
```

連線被拒絕或連不到都算 `down`；這台主機沒有通往 backend 的路由，或連線被本機的防火牆擋下，也一樣：這台主機上的應用程式同樣連不到它。只有本機資源不夠用（開啟的檔案、緩衝區、記憶體、本機 port）才算 `error`。

| 選項 | 型別 | 預設值 | 意義 |
| --- | --- | --- | --- |
| `send` | 字串 | 無 | 連上後寫出一次的位元組。`\r\n` 這類 TOML 跳脫字元會轉成對應的位元組送出。要搭配 `expect`。 |
| `expect` | 字串 | 無 | backend 送來的資料裡一定要出現的位元組，區分大小寫（最多讀 16384 位元組）。不能是空的，最長 16384 位元組。 |
| `tls` | 布林 | `false` | 連上後立刻做 TLS handshake。絕不退回明文。 |
| `tls_verify` | 布林 | `true` | 設成 `false` 就接受任何憑證（連線還是有加密）。要搭配 `tls = true`。 |
| `server_name` | 字串 | backend 的 IP | 憑證必須符合的名稱（或 IP）；如果是名稱，也會當作 SNI 送出。要搭配 `tls = true`。 |
| `ca_file` | 絕對路徑 | 系統 CA | 改成信任這個 PEM 檔裡的 CA 憑證。載入設定時，檔案裡至少要有一張能當 CA 用的憑證。要搭配 `tls = true`。 |

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
| `Connection refused (os error 111)` | 那個 port 沒有程式在 listen（就是連線錯誤本身，沒有前綴）。 |
| `timeout` | 在 `timeout` 內沒連上，或沒收到符合的回答。 |
| `expect: got "421 too busy\r\n"` | 回答裡沒有 `expect` 的內容；後面附的是收到的前 64 位元組，已經跳脫過。 |
| `send: ...`、`expect: ...` | 傳送或讀取時發生 I/O 錯誤。 |
| `tls: no answer to the TLS handshake (does the port speak TLS?)` | 這個 port 不走 TLS。 |
| `tls: connection closed during the TLS handshake (does the server have a certificate?)` | 伺服器沒有回答就直接關掉連線，例如沒有憑證的 Active Directory 在 636 port 上就會這樣。 |
| `tls: ...` | 其他 TLS 失敗，例如憑證不符。 |

## http

每次檢查會對 backend 的 IP 與 port 送一個 `GET` 請求。不跟隨重新導向，也不走 proxy。

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
| `path` | 以 `/` 開頭的字串 | `"/"` | 請求路徑，需要的話可以帶 query string。`#`、`\` 與控制字元會被拒絕；空白或其他特殊字元會用百分比編碼送出。 |
| `host_header` | 字串 | 無 | `Host` header：主機名稱或 IP 位址，可以加 `:port`，IPv6 要加中括號（`api.internal`、`api.internal:8443`、`[2001:db8::1]`）。連線還是連到 backend；用 `https` 時，主機名稱也會當作 TLS 的 server name。 |
| `expect_status` | 非空清單 | `["200-399"]` | 算 `up` 的狀態碼：可以是單一狀態碼（`200` 或 `"200"`），也可以是含頭尾的範圍（`"200-299"`），全部都要在 100–599 之間。 |
| `expect_body` | 字串 | 無 | 回應內容裡一定要出現的位元組，區分大小寫。內容最多讀 65536 位元組。不能是空的，最長 16384 位元組。 |
| `tls_verify` | 布林 | `true` | 用在 `https`：設成 `false` 就接受任何憑證。 |
| `ca_file` | 絕對路徑 | 無 | 用在 `https`：要信任的 CA 憑證 PEM 檔，給憑證由私有 CA 簽發的伺服器用。只信這個檔案，不信系統的 CA 憑證（像 `curl --cacert`）。每次檢查都會看檔案有沒有被換掉或改寫，有的話就重新讀，所以換檔不必 reload；檔案讀不到或裡面沒有憑證，設定檢查會失敗，之後才壞掉則該次檢查結果是 `error`。要搭配 `scheme = "https"`；同時設 `tls_verify = false` 則沒有作用。 |

結果會長這樣：

| Reason | 意義 |
| --- | --- |
| `unexpected status 503` | 狀態碼不在 `expect_status` 裡。不會讀內容。 |
| `expect_body: got "<first 64 bytes>"` | 狀態碼符合，但內容裡沒有那段文字。 |
| `body: ...` | 內容讀不到。 |
| `error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)` | 連線失敗。reason 從來不含請求的 URL，所以 `path` 裡的 token 不會漏到 log 裡。 |
| `timeout` | 在 `timeout` 內沒收到完整的回答。 |
| `cannot build http client: ...; https checks need the system CA certificates, or a ca_file, unless tls_verify = false`（`error`） | 在沒有 CA 憑證的主機上（Debian 與 RHEL 上是 `ca-certificates`）跑有開驗證的 `https` 檢查。從 0.7.0 起，單純的 `http` 不需要 CA 憑證。私有 CA 請放進 `ca_file`，不必裝到系統。 |

有些健康端點不管狀態好壞都回 200，只把狀態寫在內容裡，這種就要用 `expect_body`。這時 `balance = "fastest"` 用的延遲會包含讀內容的時間。

## exec

每次檢查都執行一次程式：不經過 shell，工作目錄是 `/`，沒有標準輸入。backend 透過環境變數傳進去。

```toml
[services."report.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/check_replication.sh", "--max-lag", "60"] }
```

搭配的程式：

```sh
#!/bin/sh
# /usr/local/libexec/check_replication.sh：emfor 會設定 EMFOR_IP、EMFOR_PORT、EMFOR_SERVICE。
lag=$(psql -h "$EMFOR_IP" -p "$EMFOR_PORT" -U monitor -Atc \
      "select coalesce(extract(epoch from now() - pg_last_xact_replay_timestamp()), 0)::int" postgres) \
  || { echo "cannot query $EMFOR_IP"; exit 2; }
[ "$lag" -le "$2" ] || { echo "replication ${lag}s behind"; exit 1; }
exit 0
```

（這個 script 只是範例，沒有對真正的 PostgreSQL 跑過。）

| 程式 | 結果 |
| --- | --- |
| 以 0 結束 | `up` |
| 以 1 或 2 結束 | `down`；reason 是它標準輸出的第一行（截到 200 個字元），什麼都沒印的話就是 `exit code N` |
| 以其他代碼結束 | `error`，reason 是 `exit code N` 或 `exit code N: <first line>` |
| 被 signal 終止 | `error`（`killed by signal N`） |
| 啟動不了 | `error`（`cannot run <program>: ...`） |
| 到 `timeout` 還在跑 | `down`（`timeout`）；程式和它啟動的每個行程都會被終止 |

這就是 Nagios 的慣例，差別只在 **1（WARNING）算 down**，3（UNKNOWN）算 error。標準錯誤會被丟掉。

| 選項 | 型別 | 意義 |
| --- | --- | --- |
| `command` | 字串清單，必填 | 程式和它的參數。第一個元素一定要是可執行檔的絕對路徑；每個元素都不能含 NUL。 |
| `env_pass` | 名稱清單 | 程式要額外拿到的 daemon 環境變數（[見下](#檢查程式拿到的環境)）。 |

每個 backend 每檢查一次就要啟動一個行程，成本不低。要頻繁檢查很多 backend 時，改寫一個 [plugin](07-plugins.md#自己寫-plugin) 比較好。想用現有的 Nagios 外掛（它們要在命令列帶 `-H` 與 `-p`），可以參考[第 14 章](14-integrations.md#把-nagios-外掛當成檢查)的包裝 script。

### ping 與 SCTP

emfor 沒有 ICMP 檢查，也沒有 SCTP 檢查，而且 `sip` 和 `diameter` 外掛會拒絕 `transport = "sctp"`。在有內建檢查之前，請用 `exec`。底下的東西都沒有實際跑過，只是個起點。

`exec` 的命令列是固定的，backend 是從環境變數傳進去的，所以要寫一支小 script：

```sh
#!/bin/sh
# /usr/local/libexec/check_ping.sh：backend 回應一次 ping 就算 up。
/usr/bin/ping -c 1 -W 1 "$EMFOR_IP" >/dev/null 2>&1 || { echo "no ping answer"; exit 1; }
```

port 用不到。IPv6 的 backend 要加 `-6`。套件附的 unit 沒有任何 capability，又有 `NoNewPrivileges`，所以靠檔案 capability（`cap_net_raw`）才能用的 `ping` 在這裡不能用。Linux 上的 `ping` 可以改開不需特權的 ICMP socket，但 kernel 只允許 `net.ipv4.ping_group_range` 範圍內的 group ID 這麼做；`emfor` 這個 group 必須在範圍裡（`sysctl net.ipv4.ping_group_range` 可以看）。unit 的 `RestrictAddressFamilies` 本來就允許這種 socket。用之前先跑 `runuser -u emfor -- ping -c 1 <位址>` 確認。

SCTP 的話，用任何一支「到 `$EMFOR_IP:$EMFOR_PORT` 的 association 建起來就結束代碼 0、建不起來就結束代碼 1」的程式，放進同樣的 script。主機的 kernel 要有 SCTP（Linux 上是 `sctp` 模組，有些發行版預設不讓它載入）；沒有的話程式開不了 socket，檢查會依結束代碼回報 `error` 或 `down`。用 `AF_INET` 和 `IPPROTO_SCTP` 開 socket，unit 的 `RestrictAddressFamilies` 是允許的，不過這點同樣沒有試過。這樣的檢查只能證明 association 建得起來，不能證明後面的應用程式有回應。

每一種都是每次檢查啟動一個 process，成本見上面。

## 檢查程式拿到的環境

從 0.7.0 起，`exec` 程式與 plugin **拿不到 daemon 的環境**。它們只會拿到：

- `PATH`、`LANG`、`TZ` 與所有 `LC_*` 變數，值和 daemon 的一樣；
- `exec` 另外會拿到 `EMFOR_IP`、`EMFOR_PORT` 與 `EMFOR_SERVICE`；
- `env_pass` 裡列出的變數。

所以 daemon 環境裡的祕密（例如 unit 檔設定的）不會流到檢查程式手上。程式如果需要其他變數，例如 `HOME` 或 `https_proxy`，只要列出它的名稱就會拿到：

```toml
[services."report2.svc.emfor"]
check = "exec"
backends = ["10.0.3.1:5432"]
check_options = { command = ["/usr/local/libexec/check_replication.sh", "--max-lag", "60"], env_pass = ["HOME", "PGSSLROOTCERT"] }

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]
env_pass = ["https_proxy"]
```

- `env_pass` 列的是**名稱**，絕對不是值。daemon 沒有的名稱會直接略過，不算錯誤。要給程式一個 daemon 環境裡沒有的值，就先在 unit 檔裡設定（`Environment=`），再把名稱列進來。
- 名稱只能由字母、數字與 `_` 組成，不能以數字開頭，而且只能出現一次。對 `exec` 來說，名稱不能是 `EMFOR_IP`、`EMFOR_PORT` 或 `EMFOR_SERVICE`（`env_pass: EMFOR_IP is set by emfor`）。
- `emfor-check` 做沒有 `ca_file` 的 TLS 檢查時，會讀 `SSL_CERT_FILE` 與 `SSL_CERT_DIR` 來找系統 CA。如果你靠這兩個變數，就把它們加進該 plugin 的 `env_pass`，或改設 `ca_file`。
- 改了 plugin 的 `env_pass`，reload 時該 plugin 會重新啟動。
- 在聯防群組裡，`env_pass` 是成員之間會比對的檢查定義的一部分：同一個服務在每台主機上都要列出相同的名稱。
- 程式需要卻拿不到某個變數時，`emfor check-config` 不會警告。從 0.6 升級之前，先找出哪些程式會讀 `HOME`、proxy 變數或 `LD_LIBRARY_PATH`（[第 15 章](15-upgrading.md)）。

這在 container 裡驗證過：設定 `env_pass = ["HOME"]`，並用 `HOME=/root MY_SECRET=hunter2` 啟動 daemon，檢查程式看到了 `HOME=/root`，沒看到 `MY_SECRET`。

## 排程

每個 backend 都照自己的時程檢查，同一個 backend 也不會同時有兩個檢查在跑：

- 啟動或 reload 後的第一次檢查，會在 `min(interval, 200ms)` 內的某個隨機時間點開始。
- backend 還是 `unknown`、而且檢查結果是 `up` 或 `down` 時，上一次檢查結束後 `min(interval, 200ms)` 就接著做下一次，最多 `rise + fall` 次。用預設值的話，大約半秒內每個 backend 的狀態就確定了。如果某個 backend 的結果一直變來變去，兩個計數都到不了門檻，它會維持 `unknown`，之後改成每 `interval` 檢查一次。
- 其他情況下，下一次檢查會在上一次**結束**後 `interval` 開始，並隨機提早或延後最多 `jitter`（預設 10%）。因此慢的 backend 被檢查的頻率不會高於每 `interval` 一次。
- 在聯防群組裡，不是某個 backend 負責主機的成員，檢查它的頻率會比較低（[第 11 章](11-peer-groups.md#投票如何運作)）。

## rise、fall 與 error

- `up`：成功次數加一，失敗次數歸零；到 `rise` 時 backend 變成 `healthy`。
- `down`：失敗次數加一，成功次數歸零；到 `fall` 時 backend 變成 `unhealthy`。
- `error`：什麼都不變。backend 只拿到 error、而且持續超過 `fall × interval` 時，會回到 `unknown`。

發現故障大約要 `fall × interval`（用預設值是 6 秒）；發現恢復大約要 `rise × interval`（4 秒）。每次狀態改變都會以 `info` 等級記進 log，內容有服務、backend、舊狀態與新狀態，以及 reason：

```
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason="..."
```

## reason 會被處理成安全的形式

存下 reason 之前，emfor 會先把它跳脫：tab、CR 與 LF 變成 `\t`、`\r` 與 `\n`；其他控制字元變成 `\u{1b}` 這類寫法；能把文字藏起來或打亂順序的不可見格式字元也比照處理（雙向控制字元如 U+202E、零寬字元、軟連字號、tag 字元）。超過 512 個字元的 reason 會被截斷，結尾加上 `…`。所以 backend 沒辦法讓 `emfor status` 或 log 顯示和實際情況不同的內容。

## 延遲

emfor 會替每個 backend 量每次 `up` 檢查花了多久，並保存一個平滑過的值（每個新樣本占 5% 的權重）。`emfor status` 與 `/metrics` 會顯示這個值，`balance = "fastest"` 也會用它。如果你要倚賴這個值，就選一個接近實際流量的檢查。

## 選擇 interval、timeout、rise 與 fall

| 目標 | 設定 |
| --- | --- |
| 預設值：大約 6 秒發現故障 | `interval = "2s"`、`timeout = "1s"`、`rise = 2`、`fall = 3` |
| 在快速網路上更快發現 | `interval = "500ms"`、`timeout = "300ms"`、`fall = 3`（約 1.5 秒） |
| 成本高的檢查（script、查詢） | `interval = "10s"` 或更長，`timeout` 遠低於它 |
| 偶爾會變慢的 backend | 調高 `timeout`（最多到 `interval`）與 `fall` |
| 狀態反覆變化的 backend（[第 16 章](16-troubleshooting.md#backend-反覆變化flapping)） | 調高 `rise` 與 `fall`；拿 `timeout` 和平常的回應時間比一比 |

限制：`interval` 從 `100ms` 到 `24h`；`timeout` 至少 `10ms`，最多到 `interval`；`rise` 與 `fall` 至少 1；`jitter` 從 `0%` 到 `50%`。
