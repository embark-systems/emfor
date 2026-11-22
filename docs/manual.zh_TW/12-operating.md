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

`daemon:` 那一行會是 `running`、`stale, heartbeat 4.5s ago` 或 `stopped`。有 `[peers]` 時，下面還會多一行 `peers:`（[第 11 章](11-peer-groups.md#投票如何運作)）。

| 選項 | |
| --- | --- |
| `--service NAME` | 只顯示這個服務（找不到時顯示 `service nope.svc.emfor not found`，結束代碼 2）。 |
| `--history` | 每個 backend 最近 20 次狀態變化，最新的排最前面。 |
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

JSON 物件裡有 `store`、`daemon`、`heartbeat_ms`、`heartbeat_age_ms`、`started_at_ms`、`site`、`dns_listen`、`metrics_listen`、`services`（每個服務有 `name`、`balance`、`on_all_down`、`slow_start_ms`、`all_down`，以及 `backends`，其中有 `id`、`ip`、`port`、`state`、`reason`、`checked_at`、`latency_us`、`weight`、`site`、`backup`、`recovered_at`）、`history`、`flapping`、`peers` 和 `assigned_checkers`（每個 backend 的負責主機，照排名排序；沒有聯防群組或 daemon 沒在跑時是 `null`）。

結束代碼：daemon 在跑、而且顯示的服務沒有一個是全部 down，為 0；daemon 是 stale 或 stopped，或顯示的服務裡有一個是 `ALL DOWN`，為 1；讀不到設定檔或 store，為 2。所以 `emfor status >/dev/null` 就可以拿來當這台主機上 emfor 的健康檢查。

## emfor lookup

```console
$ emfor lookup web.svc.emfor
127.0.0.1:8080
$ emfor lookup web.svc.emfor --explain
```

`--explain` 會印出 store 新不新鮮、用到哪個層級、每個候選、其他 backend 各自被排除的原因、慢啟動的步數，以及 balance 規則，而且不會推進任何 round-robin 計數器。`--key` 用來傳 affinity key；`--json` 會印出 `service`、`balance`、`on_all_down`、`family`、`via`、`store` 和 `result`。

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
```

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

## Log

daemon 把 log 寫到 standard error：在 systemd 底下會進 journal，用 FreeBSD rc script 時會進 syslog（tag `emfor`）。等級用 `[log] level` 設定（`trace`、`debug`、`info`、`warn`、`error`；預設 `info`）；想在某一次執行時改用別的等級，可以用 `RUST_LOG` 覆寫。啟動和一次狀態變化的 log 長這樣：

```text
INFO emfor_daemon::cli: emfor started: 1 services, 2 backends, store /var/lib/emfor/state.mdb
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from healthy to unhealthy (error sending request: ...) service=web.svc.emfor backend=127.0.0.2:8080 from=healthy to=unhealthy reason=...
```

| 等級 | 看到什麼 |
| --- | --- |
| `error` | 啟動不了、reload 時設定無效、store commit 失敗。 |
| `warn` | 設定警告（檔案權限、密碼不經 TLS、DNS 聽在公開位址、port 混用）、plugin 結束、損毀的 store 被移到旁邊、成員訊息被丟掉、讀不到聯防密鑰。 |
| `info` | 啟動、reload 和停止，每次狀態變化和它的 reason，plugin 的啟動和 standard error 輸出，成員變成存活或不存活，成員的通知和意見不一致。 |
| `debug` | 每一次檢查、被忽略的成員訊息。 |

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

## 在一台主機上執行多個 daemon

每個 store 只能有一個寫入者。第二個用同一個 store 的 daemon 會啟動失敗：
`cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held`
（結束代碼 1）。如果要跑第二個 daemon（測試用），請給它自己的 `[store] path`、DNS 和 metrics 位址，再用 `EMFOR_STORE` 讓它的 client 指向它的 store。
