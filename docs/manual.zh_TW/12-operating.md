# 12. 維運 emfor

[手冊](README.md) · 上一章：[11. 聯防群組](11-peer-groups.md) · 下一章：[13. 安全模型](13-security.md)

## 指令

| 指令 | 作用 |
| --- | --- |
| `emfor server` | 在前景執行 daemon。 |
| `emfor check-config [--show]` | 驗證設定（也會啟動每個 plugin，讓它驗證自己的選項）；`--show` 會印出合併後的設定，每個值後面註明是哪個檔案設的（不會補上預設值）。 |
| `emfor status [--service NAME] [--json] [--watch] [--history]` | 顯示 daemon 在 store 裡發布了哪些內容。 |
| `emfor lookup SERVICE [--key KEY] [--explain] [--json] [--dns A\|AAAA]` | 跟 client 函式庫一樣，查詢一個 backend。 |
| `emfor --version` | 印出 `emfor` 和版本號。 |

每個指令都可以加 `--config PATH`（預設是 `/etc/emfor/emfor.toml`，或編譯時內建的路徑，[第 4 章](04-installing.md#預設設定檔路徑)）。`status` 和 `lookup` 只會從設定檔讀 `[store] path`，不看 `EMFOR_STORE`。它們需要的 store 讀取權限跟應用程式一樣。

## Signal

| Signal | 效果 |
| --- | --- |
| `SIGHUP` | 重新載入設定。log 會寫 `config reloaded: 1 services, 2 backends`；失敗時寫出錯誤和 `reload rejected; keeping the previous config`。 |
| `SIGTERM`、`SIGINT` | 停止：記錄 `shutting down`，把 heartbeat 設成 0，讓 client 馬上看到 store 過期，接著停掉 plugin，以 0 結束。 |

`systemctl reload emfor` 會先跑 `emfor check-config`，通過了才送 `SIGHUP`。reload 能改哪些東西，請看[第 5 章](05-configuring.md#套用變更reload)。

## emfor status

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 1.4s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    1.1s ago  2.2ms
  127.0.0.2:8080  unhealthy  1.0s ago  -      error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
```

每個 backend 一行，依序是位址、狀態、上次檢查的時間、平滑後的檢查延遲（沒有就顯示 `-`），後面再接標記：

| 標記 | 意義 |
| --- | --- |
| `ALL DOWN`（在服務名稱後面） | 沒有任何健康的 backend。 |
| `drained`（放在狀態欄）、`(checks: healthy)` | `drain = true`；括號裡是檢查的結果。 |
| `pool` | `fastest`：在快的 pool 裡。 |
| `backup` | `backup = true`。 |
| `last check failed` | 還是 `healthy`，但最近一次檢查失敗了。 |
| `FLAPPING (5 changes in 10m)` | 最近十分鐘內狀態變了五次以上。 |
| `voted` | 這台主機不是這個 backend 的負責主機；它的狀態可能來自負責主機的投票。負責主機是哪幾台，只有 `--json` 會列出來。 |
| `local-up` | 意見不一致：投票結果是 down，但在這裡是 up。 |
| reason | 最近一次檢查失敗的原因。 |

`daemon:` 那一行會是 `running`、`stale, heartbeat 4.5s ago` 或 `stopped`。主機時鐘被往回調之後，heartbeat 可能比現在還晚：這時那一行會寫 `heartbeat 5.0s in the future`，超過 3 秒就是 `stale`。有 `[peers]` 時，下面還會多一行 `peers:`（[第 11 章](11-peer-groups.md#投票如何運作)）；有成員沒有送結果時，還會有像 `peers: no results from 10.0.1.13:5302 for api.svc.emfor (1 backends)` 的行（[第 11 章](11-peer-groups.md#沉默的負責主機)）；有成員的時鐘差了 2 秒以上時，還會有像 `peers: clock of 10.0.1.12:5302 is 3.0 s behind this host (messages more than 5 s apart are dropped)` 的行（[第 11 章](11-peer-groups.md#密鑰)）。

| 選項 | |
| --- | --- |
| `--service NAME` | 只顯示這個服務（找不到時顯示 `service nope.svc.emfor not found`，結束代碼 2）。 |
| `--history` | 每個 backend 最近 20 次狀態變化，最新的排最前面。不能和 `--json` 一起用；`--json` 的 `history` 裡本來就有。 |
| `--watch` | 全螢幕，每秒更新一次。按鍵：PgUp、PgDn 或空白鍵、上、下、Home、End、`q`。 |
| `--json` | 所有內容輸出成一個 JSON 物件，給 script 用。 |

```console
$ emfor status --history
...
web.svc.emfor
  127.0.0.1:8080
    2026-11-21 17:31:25 unknown -> healthy
  127.0.0.2:8080
    2026-11-21 17:31:31 healthy -> unhealthy  error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
    2026-11-21 17:31:25 unknown -> healthy
```

JSON 物件裡有 `store`、`daemon`、`heartbeat_ms`、`heartbeat_age_ms`（停止時是 `null`，heartbeat 比現在還晚時是負數）、`started_at_ms`、`site`、`dns_listen`、`metrics_listen`、`services`（每個服務有 `name`、`balance`、`on_all_down`、`slow_start_ms`、`all_down`，以及 `backends`，其中有 `id`、`ip`、`port`、`state`、`reason`、`checked_at`、`latency_us`、`weight`、`site`、`backup`、`recovered_at`）、`history`、`flapping`、`peers`（有成員沒有送結果時帶 `silent_checkers`，見[第 11 章](11-peer-groups.md#沉默的負責主機)；有成員的時鐘差了 2 秒以上時帶 `clock_offsets`，見[第 11 章](11-peer-groups.md#密鑰)）和 `assigned_checkers`（每個 backend 的負責主機，照排名排序；沒有聯防群組或 daemon 沒在跑時是 `null`）。

`history` 每個服務一筆，內容是 `{"backends": {"<id>": [...]}}`；每個 backend 有它最近 20 次狀態變化，**最舊的排最前面**（`--history` 的文字輸出是最新的排最前面），每筆是 `{"at": <該次檢查的 Unix 毫秒>, "from": "healthy", "to": "unhealthy", "reason": "connection refused"}`；檢查沒給原因時 `reason` 是 `null`。

結束代碼：daemon 在跑、而且顯示的服務沒有一個是全部 down，為 0；daemon 是 stale 或 stopped，或顯示的服務裡有一個是 `ALL DOWN`，為 1；讀不到設定檔或 store，為 2。所以 `emfor status >/dev/null` 就可以拿來當這台主機上 emfor 的健康檢查。

## emfor lookup

```console
$ emfor lookup web.svc.emfor
127.0.0.1:8080
$ emfor lookup web.svc.emfor --explain
```

`--explain` 會印出 store 新不新鮮、用到哪個層級、每個候選、其他 backend 各自被排除的原因、慢啟動的步數，以及 balance 規則，而且不會推進任何 round-robin 計數器。`--key` 用來傳 affinity key；`--json` 會印出 `service`、`balance`、`on_all_down`、`family`、`via`、`store` 和 `result`。主機時鐘被往回調之後，heartbeat 可能比現在還晚：這時 store 那一行會寫 `heartbeat 1m00s in the future`，JSON 裡的 `store.heartbeat_age_ms` 是負數。

`--dns A` 或 `--dns AAAA` 不會真的去查 DNS server。它讀的是同一個 store，告訴你 daemon 的 DNS server 對這個查詢會怎麼回答，而且一定會附上說明。daemon 做出判斷後，最多大約一秒才會更新 store，所以在這一秒內，實際的 DNS 回答可能不一樣；DNS server 的輪替計數器在 daemon 的記憶體裡，這裡看不到。

| 結束代碼 | 意義 |
| --- | --- |
| 0 | 印出了位址。 |
| 1 | `ALL_DOWN`；用 `--dns` 時，如果服務沒有那個位址族的 backend，也是 1（這種情況 DNS server 會回 NOERROR、沒有記錄）。 |
| 2 | 沒有這個服務，或命令列寫錯。 |
| 3 | store 不能用（`UNAVAILABLE`、`INCOMPATIBLE`）。 |

## Metrics

有 `[metrics]` table 時，daemon 會在 `http://127.0.0.1:5301/metrics` 提供 Prometheus 文字格式的 metrics（其他路徑回 404）。請只聽在 loopback 上，或放在防火牆後面：裡面會列出每個服務和 backend 的名稱。

```toml
[metrics]
listen = ["127.0.0.1:5301"]
allow = ["127.0.0.0/8", "10.20.0.0/16"]   # 選用
```

`allow` 是網段清單（`位址/前綴長度`，IPv4 或 IPv6，不能重複）。設了之後，來源不在清單內的連線會得到 `403 Forbidden`，拿不到任何 metrics；沒設就每個來源都服務。沒有例外：設了 `allow`，本機要讀就得把 `127.0.0.0/8` 列進去。daemon 看到的是直接連進來的對端位址，所以經過 NAT 或反向 proxy 時算的是 proxy 的位址，也不會讀 `X-Forwarded-For`。`allow` 改了要 restart，跟 `listen` 一樣。`listen` 有非 loopback 的位址又沒設 `allow` 時，daemon 在啟動和 reload 後都會發出警告。

| Metric | Label | 意義 |
| --- | --- | --- |
| `emfor_build_info` | `version` | 一律為 1。 |
| `emfor_start_time_seconds` | | daemon 啟動的時間。 |
| `emfor_service_all_down` | `service` | 沒有健康的 backend 時為 1。 |
| `emfor_backend_state` | `service`、`backend`、`state` | backend 目前所處的狀態為 1。 |
| `emfor_backend_drain` | `service`、`backend` | 被 drain 時為 1。 |
| `emfor_backend_check_latency_seconds` | `service`、`backend` | 平滑後的檢查時間，只算 healthy 的 backend。 |
| `emfor_backend_last_check_timestamp_seconds` | `service`、`backend` | |
| `emfor_backend_checks_total` | `service`、`backend`、`result` | 檢查次數，依結果分類（`up`、`down`、`error`）。 |
| `emfor_backend_transitions_total` | `service`、`backend`、`to` | 狀態變化次數。 |
| `emfor_store_last_commit_timestamp_seconds` | | 距離現在不應該超過一兩秒。 |
| `emfor_store_commit_failures_total` | | 應該一直是 0。 |
| `emfor_store_used_bytes`、`emfor_store_map_size_bytes` | | 在使用量逼近 map size 之前，就要提早告警。 |
| `emfor_plugin_up`、`emfor_plugin_restarts_total` | `plugin` | |
| `emfor_dns_rate_limited_total` | `action` | `truncated`、`dropped`。 |
| `emfor_peer_*` | | [第 11 章](11-peer-groups.md#metrics)。 |

現成的告警規則放在
[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml)
（[第 14 章](14-integrations.md#prometheus)）。

### 用 TLS 提供 metrics

`/metrics` 是明文 HTTP，本身沒有認證。要從別的主機抓、又不想讓它明文經過網路時，把 `listen` 留在 loopback，前面放一個終結 TLS 的反向 proxy，並要求抓取端出示憑證。proxy 從 `127.0.0.1` 連進來，所以 `allow` 只要放這一個來源：

```toml
[metrics]
listen = ["127.0.0.1:5301"]
allow = ["127.0.0.1/32"]
```

用 nginx：

```nginx
server {
    listen 8443 ssl;
    ssl_certificate         /etc/nginx/emfor/server.pem;
    ssl_certificate_key     /etc/nginx/emfor/server.key;
    ssl_protocols           TLSv1.2 TLSv1.3;
    # 只有持有這個 CA 簽發的憑證的抓取端可以連。
    ssl_client_certificate  /etc/nginx/emfor/ca.pem;
    ssl_verify_client       on;
    location = /metrics { proxy_pass http://127.0.0.1:5301/metrics; }
    location / { return 404; }
}
```

Prometheus 這邊：

```yaml
scrape_configs:
  - job_name: emfor
    scheme: https
    tls_config:
      ca_file: /etc/prometheus/emfor-ca.pem
      cert_file: /etc/prometheus/scraper.pem
      key_file: /etc/prometheus/scraper.key
    static_configs:
      - targets: ["host1:8443", "host2:8443"]
```

- 沒有 `ssl_verify_client` 時，連得到這個 port 的人都讀得到 metrics，只是加密而已。請開著它，或用防火牆把這個 port 限制給抓取端。
- 換伺服器憑證只要 `nginx -s reload`，不必動 emfor。
- 伺服器憑證要寫上抓取端連的位址（`subjectAltName`）。
- 在同一個 loopback 位址前面放 `stunnel` 或 HAProxy 也一樣。emfor 本身沒有 `/metrics` 的 TLS。

[tools/metrics-proxy-test.sh](../../tools/metrics-proxy-test.sh) 在容器裡用 Debian 的 nginx 和一個臨時 CA 試過這個做法：只有持有該 CA 憑證的抓取端拿得到 metrics，別的 CA 或沒有憑證的都拿不到，對 TLS port 說明文 HTTP 也拿不到，換掉伺服器憑證後 reload 就生效，emfor 全程沒有停。專案沒有用 Prometheus 實際抓過，也沒試過 HAProxy 或 stunnel。

## Log

daemon 把 log 寫到 standard error：在 systemd 底下會進 journal，用 FreeBSD rc script 時會進 syslog（tag `emfor`）。等級用 `[log] level` 設定（`trace`、`debug`、`info`、`warn`、`error`；預設 `info`）；想在某一次執行時改用別的等級，可以用 `RUST_LOG` 覆寫。`[log] format` 是 `"text"`（預設）或 `"json"`（見[下面](#json-行)）。啟動和一次狀態變化的 log 長這樣：

```text
INFO emfor_daemon::cli: emfor started: 1 services, 2 backends, store /var/lib/emfor/state.mdb
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason=...
```

狀態變化那一行的 `event`、`service`、`backend`、`from`、`to`、`reason` 欄位是穩定的，下面 reload 稽核行的欄位也是；行裡的措辭則不穩定。

| 等級 | 看到什麼 |
| --- | --- |
| `error` | 啟動不了、reload 時設定無效、store commit 失敗。 |
| `warn` | 設定警告（檔案權限、密碼不經 TLS、DNS 聽在公開位址、port 混用）、plugin 結束、損毀的 store 被移到旁邊、成員訊息被丟掉、讀不到聯防密鑰。 |
| `info` | 啟動、reload 和停止，每次狀態變化和它的 reason，plugin 的啟動和 standard error 輸出，成員變成存活或不存活，成員的通知和意見不一致。 |
| `debug` | 每一次檢查、被忽略的成員訊息。 |

### JSON 行

設了 `[log] format = "json"`，daemon 讀完設定之後寫的每一行 log 都是一行一個 JSON 物件，事件的欄位直接放在最上層，不會包在底下一層：

```json
{"timestamp":"2026-11-23T04:29:21.959477Z","level":"INFO","message":"backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (connection refused)","event":"backend_state_changed","service":"web.svc.emfor","backend":"127.0.0.2:8080","from":"healthy","to":"unhealthy","reason":"connection refused","target":"emfor_daemon::schedule"}
```

穩定的有：每一行的 `timestamp`（RFC 3339）和 `level`；狀態變化那一行的 `event`、`service`、`backend`、`from`、`to`、`reason`；以及下面 reload 稽核行的欄位和 `change`。不穩定的有：`message` 的文字、`target`、其他行的欄位。任何一版都可能新增欄位，所以只讀你要的，其餘忽略。

讀設定檔之前寫的行，例如啟動 plugin，或啟動時的設定錯誤，是文字：那時 daemon 還不知道格式。請把不是 JSON 的行送到別的地方，或直接忽略。`format` 只在啟動時讀：reload 改了它會被拒絕，訊息是 `log format settings require restart`；只改 `level` 則會套用。`RUST_LOG` 只改等級，不改格式。

### reload 改了什麼

daemon 在啟動和每次 reload 時，會把讀到的每個設定檔連同它的 SHA-256 記下來，值和 `sha256sum` 印的一樣，沒設 `include_dir` 也一樣。驗證失敗的 reload 也會記雜湊，所以你看得出被拒絕的是哪一版檔案：

```text
INFO emfor_daemon::cli: config file read: /etc/emfor/emfor.toml file=/etc/emfor/emfor.toml sha256=9f2c...e1
```

reload 被接受之後，每個差異各有一行 `info`，排在 `config reloaded` 之前。每行都有 `change` 欄位和下表的欄位。欄位名和 `change` 的值是穩定的，整行的文字不是。

| `change` | 其他欄位 |
| --- | --- |
| `service_added`、`service_removed` | `service`、`backends` |
| `backend_added`、`backend_removed` | `service`、`backend` |
| `backend_changed` | `service`、`backend`、`field`（`weight`、`site`、`backup`、`drain`）、`from`、`to` |
| `service_changed` | `service`、`field`（`check`、`interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`balance`、`peers`、`slow_start`、`dns_ttl`）、`from`、`to` |
| `check_definition_changed` | `service`、`digest_from`、`digest_to`、`resets_state` |
| `peer_member_added`、`peer_member_removed` | `peer` |
| `peers_changed` | `field`（`encrypt`、`accept_plaintext`、`cipher`、`secret_file`、`previous_secret_file`）、`from`、`to`、`applies` |
| `more` | `omitted` |

- 檢查定義改變時只會看到新舊定義的摘要（digest），不會看到選項本身。`resets_state=true` 表示這個服務的 backend 會從 `unknown` 重來；只改 `timeout` 時是 `false`。
- `applies=next_start` 標的是 `previous_secret_file`，它只在啟動時讀。
- 一次 reload 最多記 50 行，其餘合成一行 `change=more`，數量在 `omitted`。什麼都沒改的 reload 不會有這些行。
- `secret_file` 的**內容**換掉不算 reload：daemon 會自己發現，並記一行 `info`，帶 `change=peers_secret_rotated` 和 `secret_file=<路徑>`。只有**路徑**改變時才會有上面的 `peers_changed` 行。
- 任何一行都不含檢查選項、密鑰檔的內容，或它的雜湊。
- log 記的是改了什麼、什麼時候，不是誰改的：`SIGHUP` 不帶寄件者。要記下誰改了檔案，請用 auditd 監看設定目錄（`auditctl -w /etc/emfor -p wa -k emfor-config`），或把檔案放在設定管理工具底下，並把 daemon 的 log 送到集中的 log 系統。

## daemon 的結束代碼

| 代碼 | 意義 | 重新啟動有用嗎？ |
| --- | --- | --- |
| 0 | 正常停止。 | |
| 1 | 啟動不了：位址已經被占用（`cannot start metrics server: cannot listen on 127.0.0.1:5301 (tcp): Address already in use (os error 98)`）、開不了 store，或 store 被另一個 daemon 占著。 | 沒用 |
| 2 | 設定無效，或沒有給指令。 | 沒用 |
| 70 | 執行中發生內部故障（state writer 或某個 DNS listener 停了）。 | 有用 |

systemd unit 遇到 1 和 2 不會重新啟動；FreeBSD 的 `daemon(8)` 則是每次結束後都會在 2 秒後重新啟動。

## 磁碟上的 store

| 檔案 | 權限 | |
| --- | --- | --- |
| `state.mdb` | 0640（`[store] file_mode`） | 資料。 |
| `state.mdb-lock` | 0660（`[store] lock_file_mode`） | LMDB 的 reader table；每個讀取者都會寫這個檔案。 |
| `state.mdb.writer` | 0600 | 執行中的 daemon 會鎖住這個檔案。 |
| `state.mdb-peers` | 0600 | 有 `[peers]` 時才有：密鑰記錄。 |
| `state.mdb.corrupt` | | 被移到旁邊的損毀 store。 |

- **daemon 不會自己建立目錄。** 套件和 unit（`StateDirectory=emfor`）會建立；手動安裝時要執行 `install -d -o emfor -g
  emfor -m 0755 /var/lib/emfor`。沒有這個目錄時會看到：
  `cannot open state store: /var/lib/emfor/state.mdb.writer: No such file or directory (os error 2)`。
- daemon 用自己的執行緒 commit，至少每秒一次，每次有變化也會 commit。`sync = false`（預設）時不等磁碟寫完，所以磁碟慢也不會拖慢 heartbeat。store 裡沒有任何資料需要在當機後留下來：daemon 啟動時會重寫每個服務，只有歷史會保留。
- 啟動時如果發現 store 損毀，會把它移到 `state.mdb.corrupt`、刪掉 lock 檔，再用新的 store 啟動。執行中的應用程式會自己改用新檔案。
- 快速入門的 store 只有一個服務，回報的是 `emfor_store_used_bytes 32768`。請拿這個 metric 和 `emfor_store_map_size_bytes`（預設 64 MiB）對照著看：store 滿了以後 commit 會失敗，client 很快就會看到 store 過期。調高 `map_size` 要重新啟動。
- 絕對不要直接改 store 檔案本身（用 `cp` 蓋過去、`truncate`、`>`）：每個映射這個檔案的行程都可能因為 `SIGBUS` 掛掉。要替換的話，先在旁邊寫一個新檔案，再用 `mv` 蓋過去。

### 估算 store 要多大

store 為每個服務存一筆記錄和一份歷史（每個 backend 最近 20 次狀態變化），都是原地改寫，所以它的大小跟著 backend 的數量走，不跟檢查的次數走。`tools/scale-test.sh` 在筆電上的 Debian docker 容器裡量過（`interval = "1s"`、`timeout = "500ms"`，`tcp` 檢查打本機的 listener，最多 80 個服務）：

| backend 數 | 全部正常時的 store | 歷史填滿時的 store | 記憶體（RSS） | CPU | 切換時間 |
| --- | --- | --- | --- | --- | --- |
| 400 | 0.57 MB | 1.6 MB | 13 MiB | 5 % | 2.2 秒 |
| 2,000 | 2.7 MB | 4.4 MB | 32 MiB | 18 % | 2.8 秒 |
| 4,000 | 4.4 MB | 7.8 MB | 60 MiB | 19 % | 3.3 秒 |
| 8,000 | 7.2 MB | 17.6 MB | 114 MiB | 23 % | 3.4 秒 |

「歷史填滿」是指一半的 backend 各變過 24 次狀態之後；「切換時間」是從停掉一個 listener 到它的 backend 全部變成 `unhealthy`（上限是 `fall` × `interval` + `timeout`，這裡是 3.5 秒）。CPU 是一顆核心的百分比。

涵蓋這些數字的經驗法則：每個 backend 抓 **6 KB**，`map_size` 設成這個數字的**至少兩倍**，因為 LMDB 改一頁時會先寫到新的位置，才放掉舊的。兩倍是留的餘裕，不是量出來的。照這樣算，預設的 64 MiB 大約夠 5,000 個 backend。實際用量在自己的主機上看 `emfor_store_used_bytes`，並在它逼近 `emfor_store_map_size_bytes` 之前就告警（[store 滿了](#store-滿了)）。

這些數字來自一台主機、一個容器。需要時間的檢查（`http`、`exec`、plugin）、慢的磁碟、聯防，它們都沒有涵蓋；要靠它們之前，請在自己的硬體上測。

### 把 store 放在記憶體

store 裡沒有任何資料需要在重開機後留下來，所以可以把 store 放在 tmpfs 上，直接掛在 store 所在的目錄，大小至少要等於 `map_size`：

```text
tmpfs  /var/lib/emfor  tmpfs  size=64m,mode=0755,nosuid,nodev,noexec  0 0
```

用 `systemctl edit
emfor` 在 unit 裡加上 `RequiresMountsFor=/var/lib/emfor`。不要用 `RuntimeDirectory=`（`/run/emfor`）：systemd 每次停止服務時都會把它刪掉。這樣設定之後，每次重開機歷史都會不見。這個做法取自舊版手冊，裡面記載了在容器裡測試過；這個設定沒有為了這份手冊跑過。

## 誰能讀取 store

應用程式要能讀 `state.mdb`，還要能**寫** `state.mdb-lock`。daemon 每次啟動都會重新設定權限：

| 情況 | 做法 |
| --- | --- |
| 預設 | 檔案擁有者是 `emfor:emfor`，權限 0640 和 0660。把每個應用程式的使用者加進這個群組：`usermod -aG emfor www-data`，然後重新啟動應用程式（群組成員資格是登入時讀取的）。 |
| 每個使用者都能讀 | `file_mode = "0644"`。lock 檔還是要讓每個讀取者都能寫。 |
| 每個使用者都能讀，不靠群組 | `lock_file_mode = "0666"`。這樣一來，任何本機使用者都能透過 lock 檔干擾其他讀取者；`emfor check-config` 會對此發出警告。 |

在容器裡測過：不在群組裡的使用者會看到
`UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied`；
加入群組之後，查詢就正常了。

store 所在的目錄必須可以搜尋（`0755` 就可以），而且除了 daemon 的使用者，不應該有其他使用者能寫入。

## 重新啟動時應用程式會看到什麼

查詢從來不會等 daemon，一定是直接從 store 回答。但在重新啟動前後，會有一小段時間 store 分不出哪些 backend 好、哪些壞：

- **daemon 停止期間**：正常停止的話，store 會馬上過期（heartbeat 為 0）；如果是被 kill，要等 `EMFOR_MAX_STALENESS`（3 秒）過後才會過期。這時 `open` 的服務會回傳任何一個沒被 drain 的 backend，`closed` 的服務則查詢失敗，回傳 `ALL_DOWN`。
- **剛啟動之後**：每個 backend 在第一次檢查之前都是 `unknown`。快速檢查（每 200 ms 一次）大約半秒內就會讓狀態確定下來。

舊版手冊記載過一次量測（Debian 容器、`tcp` 檢查、立刻重新啟動）：最後一個錯誤的答案出現在送出 signal 後大約 0.25 秒，最久 0.4 秒。這次量測沒有為了這份手冊重做。

所以：

- 改設定請用 **reload**，不要重新啟動：reload 時，檢查沒改到的 backend 都會保留原本的狀態。
- 先裝好新的套件再重新啟動，不要用停止、安裝、啟動的順序。
- 一次只重新啟動一台主機；如果主機用了 `closed`、`failover` 或備援，先把它從 load balancer 裡拿掉。

## 用 drain 做滾動部署

想一台一台部署 backend，又不切斷正在處理的請求：

1. 在這台 backend 上設 `drain = true`，然後 reload。新的查詢就會跳過它。
2. 等它的連線都閒下來。emfor 看不到請求，所以這一步要你自己處理。
3. 停掉這台 backend、部署，再啟動。
4. 等它的檢查再次顯示 `healthy`（drain 期間檢查還是照常在跑）。
5. 拿掉 `drain`，再 reload。

請給這個服務一個專用的 `conf.d` 檔案，每台 backend 寫一行，因為 `backends` 清單是由設定它的那個檔案整個取代的：

```toml
# /etc/emfor/conf.d/20-api.toml
[services."api.svc.emfor"]
check = "tcp"
backends = [
  { addr = "10.0.0.11:8080" },
  { addr = "10.0.0.12:8080" },
]
```

[examples/deploy/emfor-drain.sh](../../examples/deploy/emfor-drain.sh) 負責第 1、2、4、5 步（需要 `jq`；`wait-idle` 另外需要 `ss`）：

```sh
# emfor-drain.sh on /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080        # 在 emfor 的主機上
# emfor-drain.sh wait-idle 8080                                         # 在 10.0.0.11 上
# deploy-and-restart-api
# emfor-drain.sh wait-ready 10.0.0.11:8080 120                          # 在 emfor 的主機上
# emfor-drain.sh off /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080
```

- 每台 backend 那一行必須一字不差寫成 `{ addr = "10.0.0.11:8080" },`，`}` 前面要有一個空格；遇到其他寫法的行，腳本會拒絕執行，什麼都不改。
- 腳本會用 daemon 的使用者（`EMFOR_USER`，預設 `emfor`）執行 `emfor check-config`，再用 `systemctl reload emfor` reload（想用別的指令就設 `EMFOR_RELOAD`）。檢查失敗時，它會把舊檔案放回去。
- `wait-ready` 要等到出現一次比這個指令更晚的檢查，而且檢查連續 SETTLE 秒（預設 10）都是 `healthy`，才會返回。SETTLE 必須大於 `fall × interval + timeout`。
- drain 是每台主機各自的設定：如果很多台應用程式主機都跑 emfor，請在每一台上執行 `on`（或把同一個 `conf.d` 檔案部署到每一台）。
- 一次只處理一台 backend：所有 backend 都被 drain 的服務會回答 `ALL_DOWN`。

根據 CHANGELOG，這支腳本在測試容器裡跑過；但沒有為了這份手冊跑過。

## 出了問題的時候

### 撤回一次設定變更

reload 是全有或全無，所以被拒絕的 reload 不用撤回：daemon 還在跑前一份設定，log 會寫 `reload rejected; keeping the previous config`。**被接受**的 reload 是整份套用，daemon 沒辦法自己復原。要撤回就照你改的方式倒回去：把前一版檔案放回去，再送 `SIGHUP`。

- 把設定目錄放在版本控制或設定管理工具底下；主機上沒有前一份設定的副本。
- 先對放回去的檔案跑 `emfor check-config`，unit 啟動時也是這樣做。
- reload 之後，拿 log 裡每一行 `config file read` 的 `sha256` 和這些檔案的 `sha256sum` 比對，再看稽核行（[reload 改了什麼](#reload-改了什麼)）：它們會列出這次 reload 撤掉了什麼。
- backend 的狀態和任何 reload 的規則一樣。服務的 `check` 或 `check_options` 一改，它的 backend 就從 `unknown` 重來，所以撤回時也會再來一次；只改 `drain`、`weight` 或 `peers` 則保留狀態。
- 要 restart 才生效的設定（`[store]`、各個 `listen` 位址、`[metrics] allow`）不管往哪個方向改，reload 都會拒絕：這些要 restart。

### store 滿了

`emfor_store_commit_failures_total` 一直增加，log 裡有 `MDB_MAP_FULL`：store 已經用到 `[store] map_size`。commit 會失敗，client 很快就會看到過期的 store（[第 3 章](03-concepts.md)），應用程式拿到什麼由 `on_all_down` 決定。

1. 拿 `emfor_store_used_bytes` 對照 `emfor_store_map_size_bytes`，也看看服務和 backend 的數量是不是變多了。
2. 在主設定檔把 `map_size` 調大（它不會 reload），並跑 `emfor check-config`。
3. restart。一台一台做，和一般的 restart 一樣（[重新啟動時應用程式會看到什麼](#重新啟動時應用程式會看到什麼)、[用 drain 做滾動部署](#用-drain-做滾動部署)）。

每次啟動 store 都會重建，所以調大 `map_size` 再 restart，除了 restart 那一小段空窗，不會遺失什麼。

### 時鐘跳動之後

heartbeat 和聯防訊息都是主機時鐘的時間戳，所以時鐘一跳，三個地方會看得到：

- **在主機上：** client 拿 heartbeat 和自己的時鐘比。如果時鐘往回調超過 `EMFOR_MAX_STALENESS`，`emfor status` 會寫 `heartbeat <age> in the future`，store 也算過期；daemon 還在跑的話，下一次 heartbeat（約一秒內）就恢復。daemon 沒在跑（被 kill、沒有正常停止）時，留下的 heartbeat 會一直在未來，直到時鐘追上：把 daemon 啟動起來，它會寫一個新的。
- **在聯防群組裡：** 時間戳和接收端時鐘差超過 5 秒（不論早或晚）的訊息會被當成 `stale` 丟掉。在丟掉之前，從 2 秒起，`emfor status` 就會有一行 `peers: clock of <member> is <n.n> s behind this host`（或 `ahead of`），`emfor_peer_clock_offset_seconds{peer}` 也有每個成員的偏差；如果有一行寫的是這台主機的時鐘和每個成員都不一樣，先修這台的時鐘。把時鐘修好（chrony 或 ntpd）就好，不用 restart。`emfor_peer_messages_rejected_total{reason="stale"}` 不再增加，就表示成員又對得上了。

每一台跑 emfor 的主機，時鐘請用漸進調整（chrony 的預設），不要一次跳過去。



每個 store 只能有一個寫入者。第二個用同一個 store 的 daemon 會啟動失敗：
`cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held`
（結束代碼 1）。如果要跑第二個 daemon（測試用），請給它自己的 `[store] path`、DNS 和 metrics 位址，再用 `EMFOR_STORE` 讓它的 client 指向它的 store。
