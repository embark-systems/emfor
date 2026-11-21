# 12. 維運 emfor

[手冊](README.md) · 上一章：[11. 聯防群組](11-peer-groups.md) · 下一章：[13. 安全模型](13-security.md)

## 指令

| 指令 | 作用 |
| --- | --- |
| `emfor server` | 在前景執行 daemon。 |
| `emfor check-config [--show]` | 驗證設定（並啟動每個 plugin 驗證它的選項）；`--show` 印出合併後的設定，每個值後面註明設定它的檔案（不會補上預設值）。 |
| `emfor status [--service NAME] [--json] [--watch] [--history]` | 顯示 daemon 在 store 中發布了什麼。 |
| `emfor lookup SERVICE [--key KEY] [--explain] [--json] [--dns A\|AAAA]` | 像 client 函式庫一樣查詢一個 backend。 |
| `emfor --version` | `emfor 0.7.0`。 |

每個指令都接受 `--config PATH`（預設 `/etc/emfor/emfor.toml`，或編在程式裡的路徑，[第 4 章](04-installing.md#預設設定檔路徑)）。`status` 與 `lookup` 只從中讀取 `[store] path`；它們不使用 `EMFOR_STORE`。它們需要與應用程式相同的 store 讀取權限。

## Signal

| Signal | 效果 |
| --- | --- |
| `SIGHUP` | 重新載入設定。記錄 `config reloaded: 1 services, 2 backends`，或記錄錯誤與 `reload rejected; keeping the previous config`。 |
| `SIGTERM`、`SIGINT` | 停止：記錄 `shutting down`，把 heartbeat 設為 0，讓 client 立刻看到過期的 store，停止 plugin，以 0 結束。 |

`systemctl reload emfor` 會先執行 `emfor check-config`，只有通過時才送出 `SIGHUP`。reload 能改變什麼，見[第 5 章](05-configuring.md#套用變更reload)。

## emfor status

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 1.4s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    1.1s ago  2.2ms
  127.0.0.2:8080  unhealthy  1.0s ago  -      error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
```

每個 backend 那一行有位址、狀態、上次檢查的時間、平滑後的檢查延遲（沒有時為 `-`），之後是標記：

| 標記 | 意義 |
| --- | --- |
| `ALL DOWN`（在服務之後） | 沒有健康的 backend。 |
| `drained`（在狀態的位置）、`(checks: healthy)` | `drain = true`；檢查給的狀態。 |
| `pool` | `fastest`：在快的 pool 中。 |
| `backup` | `backup = true`。 |
| `last check failed` | 仍是 `healthy`，但最近一次檢查失敗。 |
| `FLAPPING (5 changes in 10m)` | 最近十分鐘內有五次以上的狀態變化。 |
| `voted` | 這台主機不是這個 backend 的負責主機之一；它的狀態可能來自它們的投票。負責主機只有 `--json` 才會列出。 |
| `local-up` | 意見不一致：依投票為 down，在這裡是 up。 |
| reason | 最近一次檢查失敗的原因。 |

`daemon:` 那一行寫的是 `running`、`stale, heartbeat 4.5s ago` 或 `stopped`。有 `[peers]` 時，接著是一行 `peers:`（[第 11 章](11-peer-groups.md#投票如何運作)）。

| 選項 | |
| --- | --- |
| `--service NAME` | 只顯示那個服務（`service nope.svc.emfor not found`，結束代碼 2）。 |
| `--history` | 每個 backend 最近 20 次狀態變化，最新的在前。 |
| `--watch` | 每秒重繪，全螢幕。按鍵：PgUp、PgDn 或空白鍵、上、下、Home、End、`q`。 |
| `--json` | 全部內容作為一個 JSON 物件，供 script 使用。 |

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

JSON 物件有 `store`、`daemon`、`heartbeat_ms`、`heartbeat_age_ms`、`started_at_ms`、`site`、`dns_listen`、`metrics_listen`、`services`（每個服務有 `name`、`balance`、`on_all_down`、`slow_start_ms`、`all_down`，以及 `backends`，其中有 `id`、`ip`、`port`、`state`、`reason`、`checked_at`、`latency_us`、`weight`、`site`、`backup`、`recovered_at`）、`history`、`flapping`、`peers` 與 `assigned_checkers`（每個 backend 的負責主機，依排名順序；沒有聯防群組或 daemon 沒在執行時為 `null`）。

結束代碼：daemon 正在執行、且顯示的服務沒有一個全部 down 時為 0；daemon 為 stale 或 stopped，或顯示的某個服務為 `ALL DOWN` 時為 1；設定檔或 store 讀不到時為 2。所以 `emfor status >/dev/null` 就是這台主機上 emfor 的健康檢查。

## emfor lookup

```console
$ emfor lookup web.svc.emfor
127.0.0.1:8080
$ emfor lookup web.svc.emfor --explain
```

`--explain` 印出 store 是否新鮮、層級、每個候選、其他每個 backend 被排除的原因、慢啟動的步數，以及 balance 規則，而且不會推進任何 round-robin 計數器。`--key` 傳入 affinity key，`--json` 印出 `service`、`balance`、`on_all_down`、`family`、`via`、`store` 與 `result`。

`--dns A` 或 `--dns AAAA` 不會去查詢 DNS server。它讀同一個 store，說明 daemon 的 DNS server 對這個查詢會怎麼回答，而且一律附上說明。daemon 在做出判斷後最多約一秒才更新 store，所以在那一秒內，實際的 DNS 回答可能不同；DNS server 的輪替計數器在 daemon 的記憶體裡，不會顯示。

| 結束代碼 | 意義 |
| --- | --- |
| 0 | 印出了位址。 |
| 1 | `ALL_DOWN`；用 `--dns` 時，服務沒有那個位址族的 backend 也是 1（DNS server 對此回答 NOERROR、沒有記錄）。 |
| 2 | 沒有這個服務，或命令列有誤。 |
| 3 | store 無法使用（`UNAVAILABLE`、`INCOMPATIBLE`）。 |

## Metrics

有 `[metrics]` table 時，daemon 在 `http://127.0.0.1:5301/metrics` 提供 Prometheus 文字格式（其他路徑回 404）。請保持在 loopback 上或放在防火牆後面：它列出每個服務與 backend 的名稱。

```toml
[metrics]
listen = ["127.0.0.1:5301"]
```

| Metric | Label | 意義 |
| --- | --- | --- |
| `emfor_build_info` | `version` | 一律為 1。 |
| `emfor_start_time_seconds` | | daemon 啟動的時間。 |
| `emfor_service_all_down` | `service` | 沒有健康的 backend 時為 1。 |
| `emfor_backend_state` | `service`、`backend`、`state` | backend 目前的狀態為 1。 |
| `emfor_backend_drain` | `service`、`backend` | 被 drain 時為 1。 |
| `emfor_backend_check_latency_seconds` | `service`、`backend` | 平滑後的檢查時間，只有 healthy 的 backend。 |
| `emfor_backend_last_check_timestamp_seconds` | `service`、`backend` | |
| `emfor_backend_checks_total` | `service`、`backend`、`result` | 依結果分類的檢查次數（`up`、`down`、`error`）。 |
| `emfor_backend_transitions_total` | `service`、`backend`、`to` | 狀態變化次數。 |
| `emfor_store_last_commit_timestamp_seconds` | | 不應該舊於一兩秒。 |
| `emfor_store_commit_failures_total` | | 應該保持 0。 |
| `emfor_store_used_bytes`、`emfor_store_map_size_bytes` | | 在使用量接近 map size 之前就要及早告警。 |
| `emfor_plugin_up`、`emfor_plugin_restarts_total` | `plugin` | |
| `emfor_dns_rate_limited_total` | `action` | `truncated`、`dropped`。 |
| `emfor_peer_*` | | [第 11 章](11-peer-groups.md#metrics)。 |

現成的告警規則在
[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml)
（[第 14 章](14-integrations.md#prometheus)）。

## Log

daemon 把 log 寫到 standard error：systemd 下進 journal，FreeBSD rc script 下進 syslog（tag `emfor`）。`[log] level` 設定等級（`trace`、`debug`、`info`、`warn`、`error`；預設 `info`）；`RUST_LOG` 可以為某一次執行覆寫它。啟動與一次狀態變化看起來像這樣：

```text
INFO emfor_daemon::cli: emfor started: 1 services, 2 backends, store /var/lib/emfor/state.mdb
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason=...
```

| 等級 | 看到什麼 |
| --- | --- |
| `error` | 無法啟動、reload 時設定無效、store commit 失敗。 |
| `warn` | 設定警告（檔案權限、不經 TLS 的密碼、DNS 聽在公開位址、混用的 port）、結束的 plugin、被移到一旁的損毀 store、被丟棄的成員訊息、讀不到的聯防密鑰。 |
| `info` | 啟動、reload 與停止，每次狀態變化與其 reason，plugin 的啟動與 standard error，成員變成存活或不存活，成員的通知與意見不一致。 |
| `debug` | 每一次檢查、被忽略的成員訊息。 |

## daemon 的結束代碼

| 代碼 | 意義 | 重新啟動有用嗎？ |
| --- | --- | --- |
| 0 | 正常停止。 | |
| 1 | 無法啟動：位址已被使用（`cannot start metrics server: cannot listen on 127.0.0.1:5301 (tcp): Address already in use (os error 98)`）、無法開啟 store，或另一個 daemon 持有它。 | 沒用 |
| 2 | 設定無效，或沒有給指令。 | 沒用 |
| 70 | 執行中發生內部故障（state writer 或某個 DNS listener 停止了）。 | 有用 |

systemd unit 在 1 與 2 時不會重新啟動；FreeBSD 的 `daemon(8)` 在每次結束後 2 秒都會重新啟動。

## 磁碟上的 store

| 檔案 | 權限 | |
| --- | --- | --- |
| `state.mdb` | 0640（`[store] file_mode`） | 資料。 |
| `state.mdb-lock` | 0660（`[store] lock_file_mode`） | LMDB 的 reader table；每個讀取者都會寫入它。 |
| `state.mdb.writer` | 0600 | 由執行中的 daemon 鎖住。 |
| `state.mdb-peers` | 0600 | 有 `[peers]` 時：密鑰記錄。 |
| `state.mdb.corrupt` | | 被移到一旁的損毀 store。 |

- **daemon 不會建立目錄。** 套件與 unit（`StateDirectory=emfor`）會建立；手動安裝時用 `install -d -o emfor -g
  emfor -m 0755 /var/lib/emfor`。沒有這個目錄時：
  `cannot open state store: /var/lib/emfor/state.mdb.writer: No such file or directory (os error 2)`。
- daemon 在自己的執行緒上至少每秒 commit 一次，每次變化也會 commit。`sync = false`（預設）時它不等待磁碟，所以慢的磁碟不會讓 heartbeat 延遲。store 中沒有任何東西需要在當機後保留：daemon 啟動時會重寫每個服務；只有歷史會保留。
- 啟動時發現的損毀 store 會被移到 `state.mdb.corrupt`，lock 檔被刪除，daemon 以新的 store 啟動。執行中的應用程式會自行切換到新檔案。
- 快速入門的 store 只有一個服務，回報 `emfor_store_used_bytes 32768`。請拿這個 metric 與 `emfor_store_map_size_bytes`（預設 64 MiB）比較：store 滿了之後 commit 會失敗，client 很快就會看到過期的 store。調高 `map_size` 需要重新啟動。
- 絕不要原地修改 store（用 `cp` 蓋過去、`truncate`、`>`）：每個映射它的行程都可能因 `SIGBUS` 結束。要替換它，請在它旁邊寫一個檔案，再用 `mv` 蓋過去。

### 把 store 放在記憶體

store 裡沒有任何東西需要在重開機後保留，所以它可以放在直接掛在 store 目錄上的 tmpfs 上，大小至少等於 `map_size`：

```text
tmpfs  /var/lib/emfor  tmpfs  size=64m,mode=0755,nosuid,nodev,noexec  0 0
```

用 `systemctl edit
emfor` 在 unit 中加上 `RequiresMountsFor=/var/lib/emfor`。不要使用 `RuntimeDirectory=`（`/run/emfor`）：systemd 每次停止時都會刪掉它。這樣做之後，每次重開機都會遺失歷史。這個設定取自舊版手冊，其中記載了在容器中的測試；它沒有為本手冊執行過。

## 誰能讀取 store

應用程式需要 `state.mdb` 的讀取權限，以及 `state.mdb-lock` 的**寫入**權限。daemon 每次啟動都會設定權限：

| 情況 | 做法 |
| --- | --- |
| 預設 | 檔案擁有者為 `emfor:emfor`，權限 0640 與 0660。把每個應用程式使用者加進這個群組：`usermod -aG emfor www-data`，然後重新啟動應用程式（群組成員資格在登入時讀取）。 |
| 每個使用者都能讀 | `file_mode = "0644"`。lock 檔仍必須讓每個讀取者都能寫。 |
| 每個使用者都能讀，不用群組 | `lock_file_mode = "0666"`。這時任何本機使用者都能透過 lock 檔干擾其他讀取者；`emfor check-config` 會對此發出警告。 |

在容器中測試過：不在群組裡的使用者會得到
`UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied`；
加入群組之後它的查詢就能用。

store 目錄必須可以搜尋（`0755` 即可），而且除了 daemon 的使用者之外，不應該有其他使用者能寫入它。

## 重新啟動時應用程式會看到什麼

查詢從不等待 daemon；它一定從 store 回答。但在重新啟動前後的一小段時間裡，store 分不出好的和壞的 backend：

- **daemon 停止期間**：正常停止後 store 立刻過期（heartbeat 為 0）；被 kill 的話，要過 `EMFOR_MAX_STALENESS`（3 秒）之後才過期。這時 `open` 的服務回傳任何一個沒有被 drain 的 backend，`closed` 的服務以 `ALL_DOWN` 失敗。
- **剛啟動之後**：每個 backend 在第一次檢查之前都是 `unknown`。快速檢查（每 200 ms 一次）在大約半秒內就會讓狀態確定。

舊版手冊記載了一次量測（Debian 容器、`tcp` 檢查、立刻重新啟動）：最後一個錯誤答案出現在 signal 之後約 0.25 秒，最久 0.4 秒。這次量測沒有為本手冊重做。

所以：

- 改設定用 **reload**，不要重新啟動：reload 會保留每個檢查沒有改變的 backend 的狀態。
- 先安裝新的套件，再重新啟動，而不是停止、安裝、啟動。
- 一次只重新啟動一台主機；如果主機使用 `closed`、`failover` 或備援，先把它從 load balancer 中移出。

## 用 drain 做滾動部署

要一台一台部署 backend，又不切斷正在跑的請求：

1. 在 backend 上設定 `drain = true` 並 reload。新的查詢會略過它。
2. 等到它的連線都閒置。emfor 看不到請求；這一步要你自己做。
3. 停止、部署並啟動這台 backend。
4. 等到它的檢查再次顯示 `healthy`（drain 期間檢查照常進行）。
5. 拿掉 `drain` 並 reload。

請讓這個服務有自己的 `conf.d` 檔案、每台 backend 一行，因為 `backends` 清單會被設定它的檔案整個取代：

```toml
# /etc/emfor/conf.d/20-api.toml
[services."api.svc.emfor"]
check = "tcp"
backends = [
  { addr = "10.0.0.11:8080" },
  { addr = "10.0.0.12:8080" },
]
```

[examples/deploy/emfor-drain.sh](../../examples/deploy/emfor-drain.sh) 做第 1、2、4 與 5 步（它需要 `jq`，`wait-idle` 還需要 `ss`）：

```sh
# emfor-drain.sh on /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080        # 在 emfor 的主機上
# emfor-drain.sh wait-idle 8080                                         # 在 10.0.0.11 上
# deploy-and-restart-api
# emfor-drain.sh wait-ready 10.0.0.11:8080 120                          # 在 emfor 的主機上
# emfor-drain.sh off /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080
```

- 每台 backend 那一行必須寫成剛好 `{ addr = "10.0.0.11:8080" },`，`}` 之前有一個空格；腳本會拒絕其他寫法的行，並且不做任何改變。
- 它以 daemon 的使用者（`EMFOR_USER`，預設 `emfor`）執行 `emfor check-config`，並以 `systemctl reload emfor` reload（要用別的指令時設 `EMFOR_RELOAD`）。檢查失敗時它會把舊檔放回去。
- `wait-ready` 要等到有一次比這個指令更晚的檢查，而且檢查連續 SETTLE 秒（預設 10）都是 `healthy` 才返回。SETTLE 必須超過 `fall × interval + timeout`。
- drain 是各主機自己的設定：很多台應用程式主機都跑 emfor 時，請在每一台上執行 `on`（或把同一個 `conf.d` 檔案部署到每一台）。
- 一次只處理一台 backend：每個 backend 都被 drain 的服務會回答 `ALL_DOWN`。

CHANGELOG 記載這支腳本曾在測試容器中執行過；它沒有為本手冊執行過。

## 在一台主機上執行多個 daemon

每個 store 只有一個寫入者。使用相同 store 的第二個 daemon 會失敗：
`cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held`
（結束代碼 1）。要執行第二個 daemon（測試用）時，給它自己的 `[store] path`、DNS 與 metrics 位址，並用 `EMFOR_STORE` 讓它的 client 指向它。
