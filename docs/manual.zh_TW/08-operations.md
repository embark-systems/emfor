# 8. 維運

[手冊](README.md) · 上一章：[7b. DNS 做法](07-dns-recipes.md) · 下一章：[9. 疑難排解](09-troubleshooting.md)

## 指令

| 指令 | 作用 |
| --- | --- |
| `emfor server` | 在前景執行 daemon。 |
| `emfor check-config [--show]` | 驗證設定檔，有效時結束代碼為 0，無效時為 2。加 `--show` 時印出合併後的設定與每個值的來源檔案（[第 3 章](03-configuration.md#檢查設定檔)）。 |
| `emfor status` | 顯示執行中的 daemon 發布了什麼。 |
| `emfor lookup <service>` | 印出 client 會得到的 backend，加上 `--explain` 說明原因（[第 6 章](06-clients.md#從-shell-查詢emfor-lookup)）。 |
| `emfor --help` | 列出指令與預設設定檔路徑。 |

`--config <path>` 可放在指令之前或之後。不帶指令的 `emfor` 只印出用法並以 2 結束；它絕不會啟動 daemon。

## Signal

| Signal | 效果 |
| --- | --- |
| `SIGHUP` | 重新載入設定（主設定檔與 `include_dir` 的檔案）。只有整份設定都有效時才套用；否則記錄錯誤並繼續使用舊設定。見 [reload 會改變什麼](03-configuration.md#reload-會改變什麼)。 |
| `SIGTERM`、`SIGINT` | 停止：檢查結束、heartbeat 設為 0（client 立刻看到過期的 store）、停止 plugin、以 0 結束。還在載入設定的 reload 或啟動會被放棄。 |

請透過服務管理工具而不是直接送 signal：`systemctl reload emfor` 與 `service emfor reload` 會先檢查設定，設定無效時不會對 daemon 送 signal。

## emfor status

```console
$ emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.4s ago, up 3h12m, site tpe1, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

api.svc.emfor  round-robin  on_all_down=open
  10.0.0.11:8080  healthy    0.8s ago  1.2ms
  10.0.0.12:8080  unhealthy  0.9s ago  -      connect: Connection refused
```

`emfor status` 只讀取設定檔的 `[store]` table（[第 3 章](03-configuration.md#store)），並像 client 一樣以唯讀方式開啟 store。開啟 store 時會在 lock 檔登記一個 reader，所以使用者需要 store 的讀取權限與 lock 檔的寫入權限（[誰能使用 store](#誰能使用-store)）；lock 檔是 0444 時，`emfor status` 會以 `cannot open store …: Permission denied` 停止。

- **daemon**：heartbeat 不超過 3 秒時為 `running`，超過時為 `stale`（daemon 卡住或已死），正常停止後為 `stopped`。`stale` 或 `stopped` 時，顯示的服務只是 daemon 最後寫入的內容。
- **服務**：`balance` 與 `on_all_down`（[第 5 章](05-backend-selection.md#balance-方式)），沒有 backend 健康時顯示 `ALL DOWN`。
- **backend**：狀態、距上次檢查多久、平滑後的延遲，之後依序是適用的標記：`fastest` 服務中快的 backend 標示 `pool`；備援 backend 標示 `backup`（見[第 5 章](05-backend-selection.md#備援-backend)）；聯防群組裡的 `local-up` 與 `voted`（見[聯防群組](#聯防群組)）；10 分鐘內在 healthy 與 unhealthy 之間變化 5 次以上的 backend 標示 `FLAPPING (<n> changes in 10m)`；仍是 `healthy`、但最近一次檢查失敗的 backend 標示 `last check failed`（連續失敗 `fall` 次才會變成 unhealthy；在那之前，查詢通常會略過它，見[檢查開始失敗的 backend](05-backend-selection.md#檢查開始失敗的-backend)，但所有 healthy backend 都在失敗時例外）；最後是最近一次失敗檢查的 reason。設定檔裡 drain 的 backend（見[第 5 章](05-backend-selection.md#drain-一個-backend)）在狀態的位置顯示 `drained`，而 `(checks: <狀態>)`（檢查給的狀態）是最後一個標記，排在 `backup`、`local-up`、`voted` 與 `FLAPPING` 之後；它沒有 `last check failed`，也沒有最近一次失敗檢查的 reason。一行同時有所有標記、reason 又短時約 100 欄寬（十台主機、40 個 backend 的群組中量到 97 欄）；`--watch` 會截斷超過終端機寬度的行。

```
  10.0.1.3:5432   healthy  0.8s ago  1.2ms  voted  last check failed  connect: Connection refused
  10.0.1.40:5432  healthy  0.8s ago  1.2ms  backup  voted
  10.0.0.13:8080  drained  0.8s ago  1.2ms  (checks: healthy)
  127.0.0.1:9     drained  0.4s ago  -      (checks: unhealthy)
```

| 選項 | 效果 |
| --- | --- |
| `--service <name>` | 只顯示一個服務。 |
| `--json` | 一個 JSON 物件，供 script 使用。 |
| `--watch` | 每秒重繪、標示變化，並列出最近 10 次變化。`store:` 與 `daemon:` 兩行固定在上方，清單超過一個畫面時用 PgUp/PgDn、上下方向鍵與 Home/End 捲動；超過終端機寬度的行會被截斷。按 `q` 或 Ctrl-C 結束。 |
| `--history` | 列出每個 backend 最近 20 次狀態變化（重新啟動後仍保留）。 |

| 結束代碼 | 何時 |
| --- | --- |
| 0 | daemon 正在執行，且顯示的每個服務都有健康的 backend。 |
| 1 | daemon 為 stale 或 stopped，或顯示的某個服務為 `ALL DOWN`。 |
| 2 | 無法讀取設定檔或 store、store 格式版本不同、`--service` 指定的服務不存在，或無法寫出輸出。 |

pipe 提早關閉（`emfor status | head -1`）不會改變結束代碼。store 檔案被換掉時（例如 daemon 啟動時替換損壞的 store），`--watch` 會重新開啟 store。

單一不健康的 backend 不影響結束代碼，所以 `emfor status`（或 `emfor status --service <name>`）可以直接作為 cron 或 Nagios 類系統的監控檢查。更多內容見 [docs/deploy/daemon.md](../deploy/daemon.md#emfor-status)。

## Metrics

有 `[metrics]` table 時，daemon 在 `http://127.0.0.1:5301/metrics` 提供 Prometheus metrics。這個 endpoint 沒有認證也沒有 TLS；請保持在 loopback 上，或用防火牆保護。

| Metric | 意義 |
| --- | --- |
| `emfor_backend_state{service,backend,state}` | 目前狀態為 1 |
| `emfor_backend_check_latency_seconds` | 平滑後的檢查延遲（healthy 的 backend） |
| `emfor_backend_last_check_timestamp_seconds` | 上次檢查結束的時間 |
| `emfor_backend_in_pool` | 在 `fastest` 服務的 pool 中為 1 |
| `emfor_backend_drain` | 設定檔裡 drain 的 backend 為 1，其他為 0；`emfor_backend_state` 仍是檢查給的狀態 |
| `emfor_service_all_down` | 沒有健康的 backend 時為 1 |
| `emfor_backend_checks_total{result}` | 依結果分類的檢查次數：`up`、`down`、`error` |
| `emfor_backend_transitions_total{to}` | 依新狀態分類的狀態變化次數 |
| `emfor_store_commit_failures_total` | 失敗的 store commit 次數 |
| `emfor_store_last_commit_timestamp_seconds` | 上次成功 commit 的時間；應在幾秒以內 |
| `emfor_store_used_bytes`、`emfor_store_map_size_bytes` | store 使用量與 `[store] map_size` |
| `emfor_plugin_up`、`emfor_plugin_restarts_total` | plugin 健康狀況 |
| `emfor_dns_rate_limited_total{action}` | 有 `[dns]` 時：超過 `rate_limit` 的 UDP 回答，依改送什麼分類：`truncated` 或 `dropped`（[第 7 章](07-dns.md#速率限制)） |
| `emfor_peer_up{peer}` | 有 `[peers]` 時：該成員存活時為 1 |
| `emfor_peer_messages_rejected_total{reason}` | 有 `[peers]` 時：依原因分類的丟棄訊息數：`malformed`、`auth`（密鑰錯誤，或無法解密的訊息）、`sender`（不是成員，或偽造的位址）、`site`、`stale`（時鐘相差超過 5 秒）、`replay`、`plaintext`（`accept_plaintext = false` 時收到的未加密訊息） |
| `emfor_peer_notifications_total{direction}` | 有 `[peers]` 時：故障通知 `sent`（每個成員算一次）與 `received` |
| `emfor_peer_results_total{direction}` | 有 `[peers]` 時：負責主機的檢查結果 `sent`（每個成員算一次）與 `received` |
| `emfor_peer_disagreements` | 有 `[peers]` 時：群組投票判為 down、而本機檢查通過的 backend 數 |
| `emfor_peer_checks_skipped_total` | 有 `[peers]` 時：因兩台負責主機意見一致而沒有做的檢查次數 |
| `emfor_peer_queue_dropped_total` | 有 `[peers]` 時：peer task 落後超過 4096 則訊息，因而沒送出的 `down` 與 `result` 訊息數（log 記為 `the peer task is behind`）；應該保持 0 |
| `emfor_build_info{version}`、`emfor_start_time_seconds` | daemon 本身 |

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml) 提供告警規則：endpoint 無法抓取、服務 `ALL DOWN`、store commit 失敗、backend 不健康達 10 分鐘、flapping、大多是 error，或不再被檢查。[docs/deploy/daemon.md](../deploy/daemon.md#metrics) 列出每個 metric 的型別與 label。

### 聯防群組

聯防群組的成員之間需要能以 UDP 連到彼此的 [`[peers]`](03-configuration.md#peers) 的 `listen` port（範例中為 5302），時鐘相差不能超過 5 秒（NTP）。某個成員的 `emfor_peer_up` 一直是 0 時，看兩邊的 `emfor_peer_messages_rejected_total`：`auth` 表示密鑰不同，`stale` 表示時鐘偏移，`sender` 表示 `members` 或 `listen` 不符，`site` 表示兩台主機的 `site` 不同。0.3.0 成員那邊的 `malformed` 表示它讀不了已升級成員的加密訊息：在它升級前，已升級的成員先設 `encrypt = false`（見 [`[peers]`](03-configuration.md#peers)）。警告 `peer <address> sends plaintext messages (EMP1); set encrypt = true there` 表示該成員仍送未加密的訊息；這些訊息照常接受，除非設了 `accept_plaintext = false`：那時它們算成 `plaintext`，該成員在這裡不算存活。

原因會說明 `down` 從哪裡來：`peers 2/3: <原因>` 表示三台負責主機中有兩台看到 backend down（原因是它們的）；`local: <原因>` 表示只有這台主機看到 down，投票沒有判 down；沒有前綴的原因表示沒有投票（存活成員少於三台，或沒有夠新的結果）。`emfor status` 在 daemon 那一行下面顯示群組狀態：

```
peers: 2/3 live, not live 10.0.0.7:5302
peers: check definition of api.svc.emfor differs at 10.0.0.6:5302
```

群組投票判為 down、而本機檢查通過的 backend 會標上 `local-up`。這些值得看一下：這台主機連得到其他主機連不到的 backend（常見原因是網路分割，或其他主機上的防火牆規則），`emfor_peer_disagreements` 會計算它們的數量。要看某個 backend 是由哪幾台決定的，請看 `emfor status --json` 裡它的 `assigned_checkers`（見下）。「check definition differs」那一行表示該成員這個服務的 `check`、`check_options` 或 `timeout` 和這台主機不同，所以它的結果不計入；請讓設定一致。同樣的資料也在 store 的 `meta/peers` key，以及 `emfor status --json` 的 `peers` 中。其中 `services_off` 列出這台主機不參加群組的服務（`peers = false`）；它們的 backend 不會標 `voted`，也沒有 `assigned_checkers`。`members` 的每一項也有自己的 `services_off`：該成員宣告不參加、而這台主機也有的服務。這台主機挑這些服務的負責主機時不算那個成員，`emfor status` 也照同樣方式排除。文字輸出會在 `peers:` 那一行之後，為每個這樣的成員加一行，只列出目前顯示的服務：

```
peers: 10.0.0.6:5302 leaves out api.svc.emfor db.svc.emfor
```

這台主機加上沒有略過某個服務、且仍 live 的成員不到三台時，群組無法為那個服務投票：它的 backend 都不會是 voted，每個 reason 都是這台主機自己判的，沒有 `peers d/f:` 前綴。如果每個有參加群組的服務都是這種情形，`peers:` 那一行會以 `too few live members to vote` 結尾：

```
peers: 1/2 live, not live 10.0.0.7:5302, too few live members to vote
```

如果有些服務投得了票、有些投不了，這一行不會有提示；在 `assigned_checkers`（見下）裡沒有 `checkers` 的 backend，就是只由這台主機決定。

四台以上的群組裡，不是某些 backend 負責主機的主機，`emfor_peer_checks_skipped_total` 會增加：大部分的檢查時間讓投票決定，大約每三個 `interval` 才自己檢查那些 backend 一次。`emfor status` 會把那些 backend 標上 `voted`，並在 `peers:` 那一行最後寫出顯示的 backend 中有幾個是這台主機每次都檢查的：

```
peers: 4/4 live, checks 12/40 backends here
```

`emfor status --json` 在 `assigned_checkers` 中依排名列出每個 backend 的負責主機：

```json
"assigned_checkers": [
  {"service": "api.svc.emfor", "backend": "10.0.0.11:8080",
   "checkers": ["10.0.0.5:5302", "10.0.0.8:5302", "10.0.0.6:5302"]}
]
```

daemon 沒有執行或沒有 `meta/peers` 時它是 `null`。status 依 `meta/peers` 中的存活成員算出這些內容，而 daemon 每秒更新一次 `meta/peers`，所以成員剛加入或離開時最多會落後一秒。`voted` 只表示這台主機不是負責主機；某次檢查時間是否由投票決定，要看當下的票。`emfor_backend_checks_total` 仍然計算每一次檢查時間，不論是否由投票決定。backend 的延遲只由這台主機自己的檢查量測，所以在這種主機上大約每三個 `interval` 才變一次。`voted` 的 backend 那一行的「`<age> ago`」是結果套用的時間，不論由誰決定，所以它可能很新，而這台主機實際連線到那個 backend 可能是大約三個 `interval` 之前。

## Log

daemon 把 log 寫到 stderr：systemd 下進 journal，FreeBSD rc.d script 下進 syslog（tag `emfor`）。以 `[log] level` 設定等級，或用 `RUST_LOG`（例如 `RUST_LOG=debug`）只為某次執行設定。只有 stderr 是終端機且未設定 `NO_COLOR` 時才會使用顏色。

| 等級 | 看到什麼 |
| --- | --- |
| `error` | store commit 失敗、被拒絕的 reload |
| `warn` | plugin 結束、損毀的 store 被替換、在 DNS zone 之外或 port 不一致的服務、無法解析的 plugin 輸出、被丟棄的聯防訊息（每個來源與原因每分鐘一行）、`encrypt = true` 時送明文（`EMP1`）訊息的成員（每個成員每分鐘一行）、無法讀取的 `[peers] secret_file` |
| `info` | 啟動與停止、每次 backend 狀態變化與其 reason、plugin 的 stderr 內容、聯防成員變成存活或不存活、`peer … reports backend … down (…); checking now`、`backend … of … is down at 2/3 peers but up here` |
| `debug` | 每次通過的檢查所花的時間、被忽略的聯防通知與原因、檢查定義不同的成員送來的結果 |

## 結束代碼

| 代碼 | 意義 | 重試有用嗎？ |
| --- | --- | --- |
| 0 | 正常停止（`SIGTERM` / `SIGINT`） | — |
| 1 | 無法啟動：listen 位址已被使用、無法開啟 store，或另一個 emfor 持有這個 store（`<store>.writer`） | 沒用；先排除原因 |
| 2 | 設定無效，或沒有給指令 | 沒用；先用你要啟動的那個 binary 執行 `emfor check-config`，再修正設定 |
| 70 | state writer 或 DNS listener 在執行中停止了 | 有用；systemd unit 會重新啟動它 |

## 磁碟上的 store

daemon 至少每秒 commit 一次 store，每次狀態變化也會 commit。預設（`[store] sync = false`）commit 不等待磁碟，所以磁碟卡住時不會讓 heartbeat 延遲、使主機上所有應用程式都看到過期的 store。store 中沒有任何東西需要在主機當機後保留：daemon 啟動時會重寫每個服務。只有狀態歷史（`emfor status --history`）會跨重新啟動保留。

斷電後 store 可能損毀。這時 daemon 會記錄警告、把它移到 `<store>.corrupt`，並以新的 store 啟動；執行中的應用程式會自行跟上新檔案。唯一遺失的是狀態歷史，也就是 `emfor status --history` 顯示的每個 backend 最近 20 筆狀態變化。每次狀態變化也都記在 daemon 的 log 裡（`changed from healthy to unhealthy` 等），要查當機前發生了什麼，請看 log。設定 `sync = true` 會讓每次 commit 都寫入磁碟，但要承擔上述的風險。

不要原地修改 store 檔：不要用 `cp` 蓋過去、不要 `truncate`、也不要用 shell 重導向（`> state.mdb`）。daemon 和所有應用程式都透過記憶體映射讀它，檔案在它們開著時變短，這些 process 都會因 `SIGBUS` 結束。要換成另一個檔案，請先寫在 store 旁邊，再用 `mv` 蓋到 store 的路徑上，執行中的應用程式就會跟上新檔案。開檔時就已經比 LMDB 記錄的長度短的 store（例如斷電後）會當成損壞處理：daemon 把它移到 `<store>.corrupt`；在有正常的 store 之前，查詢回 `UNAVAILABLE`（訊息含 `truncated`）；`emfor status` 結束碼為 2。LMDB 本身也可能讓 store 比它記錄的短幾頁（頁面在寫入前又被釋放）；daemon 每次 commit 後都會把檔案補足長度，所以這只影響舊版 emfor 寫出的 store：升級後第一次啟動時可能被移走，只損失狀態歷史。

commit 在自己的執行緒上執行，所以慢的磁碟不會拖慢 DNS 回答、metrics 或檢查。請留意 `emfor_store_used_bytes` 與 `map_size` 的比例：store 滿了之後 commit 會失敗，client 很快就會看到過期的 store。

### 把 store 放在記憶體

store 裡沒有任何東西需要在重開機後保留，所以可以放在 tmpfs 上。這樣大約每秒一次的 commit 都不會寫到磁碟，就算設定 `sync = true`，磁碟卡住也不會延遲 commit；斷電後也不會留下損壞的檔案，daemon 只會以新的 store 啟動。代價是狀態歷史：每次重開機都會清空，不只是當機時（只重啟 daemon 會保留）。

把 tmpfs 直接掛在 store 所在的目錄（`/var/lib/emfor`；用 FreeBSD port 時是 `/var/db/emfor`），這樣 `[store] path` 和 `EMFOR_STORE` 都不用改。大小至少要等於 `[store] map_size`（預設 `64MiB`）；只有實際用到的頁面會佔記憶體，幾個服務只需要幾十 KiB。

- systemd：在 `/etc/fstab` 加上

  ```
  tmpfs  /var/lib/emfor  tmpfs  size=64m,mode=0755,nosuid,nodev,noexec  0 0
  ```

  並用 `systemctl edit emfor` 在 `[Unit]` 下加上 `RequiresMountsFor=/var/lib/emfor`，讓 daemon 等掛好之後才啟動。`StateDirectory=emfor` 每次啟動時都會把目錄交給 `emfor` 使用者。
- FreeBSD：在 `/etc/fstab` 加上

  ```
  tmpfs  /var/db/emfor  tmpfs  rw,size=64m,mode=0755,uid=<emfor 的 uid>  0 0
  ```

  uid 填 `id -u emfor` 印出的數字，讓每次開機後目錄都屬於 `emfor`。

不要把 store 放在 systemd 的 `RuntimeDirectory=`（`/run/emfor`）：daemon 每次停止時它都會刪掉整個目錄，連重啟 daemon 都會清空歷史，執行中的應用程式也會停在已刪除的檔案上。

這已在 Debian 容器中、`/var/lib/emfor` 掛上 tmpfs 的情況下驗證過：一個服務的 store 佔 32 KiB，`emfor status` 讀得到，重啟 daemon 後歷史仍在。上面的 systemd 與 FreeBSD 設定尚未在實機上跑過。

## 誰能使用 store

應用程式讀取資料檔，並在 lock 檔 `<store>-lock` 中登記讀取，所以每個應用程式使用者都需要 store 的讀取權限與 lock 檔的寫入權限。預設值（`file_mode = "0644"`、`lock_file_mode = "0666"`）讓每個本機使用者都能這麼做，但也讓任何能寫 lock 檔的本機使用者可以卡住 daemon 的 commit 或干擾查詢。只有在每個本機使用者都可信任的主機上才保留預設值。否則，為你的應用程式建立一個群組：

1. 建立一個群組，例如 `emfor-clients`，把每個應用程式使用者加進去。
2. 把它設為 daemon 的群組。systemd 下：`systemctl edit emfor`，加上 `Group=emfor-clients` 並設定 `StateDirectoryMode=0750`。FreeBSD 用 port 時：`chgrp emfor-clients /var/db/emfor && chmod 0750 /var/db/emfor`。
3. 在 `[store]` 中設定 `file_mode = "0640"` 與 `lock_file_mode = "0660"`。
4. 對既有的 store 與 lock 檔執行 `chgrp emfor-clients`，然後重新啟動 daemon（它每次啟動都會設定權限）。

## 用 drain 做滾動部署

要一台一台部署一個 service 的 backend，又不想切斷正在跑的請求：先用 `drain = true`（[第 5 章](05-backend-selection.md#drain-一個-backend)）把那台 backend 從選擇中拿掉，等它的請求結束，停止、部署、啟動，等它的檢查通過，再放回去。emfor 負責第一步和最後一步，中間顯示檢查的結果；等待要你自己做，因為 emfor 不在資料路徑上，看不到任何一個請求。

`conf.d` 的檔案無法只改別的檔案已定義的 service 的某一台 backend：清單是整個被取代，所以那個檔案要把整份 `backends` 清單重寫一遍。請讓 service 有自己的檔案、每台 backend 一行，然後改那一行：

```toml
# /etc/emfor/conf.d/20-api.toml
[services."api.svc.emfor"]
check = "tcp"
backends = [
  { addr = "10.0.0.11:8080" },
  { addr = "10.0.0.12:8080" },
]
```

[examples/deploy/emfor-drain.sh](../../examples/deploy/emfor-drain.sh) 做一台 backend 的這幾步。開始之前：

- 沒有任何套件會安裝這支腳本。請複製到 emfor 的主機（給 `on`、`off`、`wait-ready` 用）與 backend 的主機（給 `wait-idle` 用）。
- 在 emfor 的主機上以 root 執行：以 root 執行的 `emfor check-config` 只接受 root 擁有的 conf.d 檔案，而且腳本會 reload emfor。
- 需要 `jq`；`wait-idle` 還需要 `ss`（Linux、iproute2）。
- reload 指令是 `systemctl reload emfor`；要用別的，設 `EMFOR_RELOAD`（FreeBSD：`service emfor reload`，檔案在 `/usr/local/etc/emfor/conf.d`）。
- 它呼叫 `emfor check-config` 與 `emfor status` 時不帶 `--config`，所以用的是編在 emfor 裡的預設設定檔路徑。要用別的路徑，設 `EMFOR_CONFIG=/path/to/emfor.toml`。
- 每台 backend 必須是一行，寫成剛好 `{ addr = "10.0.0.11:8080" },`，`}` 之前有一個空格。遇到它改不了的那一行，腳本會停下來、不動檔案。它把新的那一行寫到一份副本，用副本取代原檔，執行 `check-config`，檢查被拒絕就把舊檔放回去；之後才 reload。
- 它用位址在 `emfor status --json` 裡找 backend，找到的是名稱順序第一個有那個位址的 service。兩個 service 共用一個位址時，設 `EMFOR_SERVICE=<name>`。

```sh
# 在執行 emfor 的主機上：在 backend 那一行加 drain = true、執行 emfor check-config、
# reload，等到 `emfor status` 顯示它被 drain。
emfor-drain.sh on /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080

# 在 10.0.0.11 上：等到連到 8080 埠的連線都有 3 秒沒有傳輸資料。
emfor-drain.sh wait-idle 8080
deploy-and-restart-api

# 回到 emfor 的主機：等到這台 backend 的檢查連續 10 秒都是 healthy，再拿掉 drain 並 reload。
emfor-drain.sh wait-ready 10.0.0.11:8080 120
emfor-drain.sh off /etc/emfor/conf.d/20-api.toml 10.0.0.11:8080
```

`wait-ready` 保證什麼：剛停掉的 backend 要等 `fall` 次檢查失敗之後才不再是 `healthy`（預設 `interval` 2 秒、`fall` 3，約 6 秒），而被 drain 的 backend 會藏起最近一次檢查的結果（store 只顯示檢查給的狀態）。所以 `wait-ready ADDR [秒數 [SETTLE]]` 不會一看到 `healthy` 就返回：要有一次比這個指令更晚完成的檢查，而且檢查連續 SETTLE 秒（預設 10）都是 `healthy` 才返回；沒有被 drain 的 backend 還要求最近一次檢查通過。SETTLE 必須比該 service 的 `fall` x `interval` 加 `timeout` 長；改了那些設定就要調大。它不計算 `rise`，也不看應用程式怎麼處理請求：通過 `tcp` 檢查、卻回 500 的 backend，它仍視為準備好了。

一次只處理一台，兩台之間看一下 `emfor status`（service 是 `ALL DOWN` 時它的結束狀態是 1）：backend 全被 drain 的 service 什麼也不回答。`drain` 是單一主機設定的一部分（[第 5 章](05-backend-selection.md#drain-一個-backend)），所以每台應用程式主機都跑 emfor 時，第一步與最後一步要在每一台上做。把好幾台主機的 emfor 都當作 name server 的 HAProxy，這幾台的 `drain` 也必須一致，而且只用一個本機的 name server 比較好（[第 7 章的 HAProxy](07-dns-recipes.md#搭配-haproxy-使用)）。

等待是為了什麼，以下是 `tests/deploy/run.sh` 在 nginx 1.30.6 與 1.31.8（用 emfor 的 SRV 回答當 upstream，[第 7 章](07-dns-recipes.md#搭配-nginx-使用)）看到的：

- backend 被 drain 時正在跑的請求會跑完（一個 1200 位元組、約 6 秒傳完的回應完整收到）。不 drain 就停掉 backend，同一個請求在 146 位元組處被切斷。
- 新的請求在幾秒內就不再到被 drain 的 backend（nginx 每秒重新查詢一次）。讀 store 的 client 是下一次 lookup；問 DNS 的 client 可能把答案快取到 TTL 結束。
- nginx 會重試被停掉的 backend 拒絕的請求，所以不 drain 就停止，在測試裡沒有丟掉新的請求（64 到 96 個 POST 中 0 個）。drain 的好處在於正在跑的請求，以及不會重試的 client；後者沒有試過。
- proxy 為了下一個請求而保持開著的連線，drain 之後仍然開著，所以「連到那個埠的連線歸零」不會來：nginx 1.30 預設會保持與 upstream 的閒置連線，等待結束時還有 1 到 2 條開著。因此 `wait-idle` 等的是一段安靜的時間，預設 3 秒（第三個參數，單位毫秒），資料取自 `ss -i`。一個沉默超過這段時間的請求（long poll 或事件串流）在它看來像是閒置：把時間調長，或在應用程式裡計算請求數。
- 預設的 `interval` 與 `fall` 下的 `wait-ready`：測試停掉一台被 drain 的 backend，馬上看 `wait-ready`；那時檢查仍說 `healthy`，而 `wait-ready` 要等 backend 重新啟動、檢查連續通過 10 秒之後才返回（停止後 26 秒，backend 是在停止後 12 秒啟動的）。測試設定檔的其餘部分用 `interval = "200ms"` 與 `fall = 1`，停掉的 backend 在不到一秒內就顯示 `unhealthy`，這些時間值會藏起這個問題，所以這個案例用預設值。測試也檢查：腳本改不了那一行時不動檔案，`check-config` 拒絕新檔案時把舊檔放回去。
- `off` 讓 backend 立刻回到選擇中，不重新計算 `rise`，也不啟動慢啟動；部署期間被重啟、在 drain 時恢復的 backend，從它的檢查通過那一刻起升權，所以很短的部署之後它可能還在 `slow_start` 之內（[第 5 章](05-backend-selection.md#慢啟動slow-start)）。

沒有試過：HAProxy（它同樣會重試，也有自己的 session）、HTTPS、自己維護連線池到 backend 的 client（drain 不會關掉池裡的連線）、多台 emfor 主機。腳本與測試只在 Linux 上跑過。腳本的較早版本（`wait-ready` 還不會等穩定、也還沒有 `EMFOR_CONFIG`、`EMFOR_SERVICE` 與放回舊檔之前）在 FreeBSD 15.1（套件裡的 nginx 1.30.6 與 `jq`）上，把 `wait-idle` 換成固定等 8 秒，手動執行測試通過；現在的腳本沒有在那裡跑過。`wait-idle` 需要 Linux 的 `ss`，在別處會直接停下並說明：請等一段固定時間，至少是最長的請求加上 proxy 重新查詢所需的時間，或在應用程式裡計算請求數。`tests/deploy/inside.sh` 本身也呼叫 `ss`、並寫入 `/etc/nginx/nginx.conf`，所以在 FreeBSD 上跑它需要先改。Integrations workflow 每週在 Linux 上執行一次，測試或腳本有改動時也會執行。

## 從 Nagios、Icinga 或 Zabbix 監看 emfor

兩支小腳本把 emfor 接到使用 Nagios 外掛慣例的監控系統（結束狀態 0 OK、1 WARNING、2 CRITICAL、3 UNKNOWN，第一行文字，`|` 後面是 perfdata）。它們是給你複製的範例，在 [examples/nagios/](../../examples/nagios/check_emfor.sh)。

**監看 emfor：`check_emfor.sh`。** 它讀 `emfor status --json`（需要 `jq`），給出 Nagios 的狀態：

```console
$ check_emfor.sh -s web.svc.emfor
EMFOR WARNING - web.svc.emfor: 2 of 3 healthy (127.0.0.1:7002 WARNING - slow on 7002) | 'web.svc.emfor_healthy'=2;3;1;0;3
$ echo $?
1
```

- CRITICAL：daemon 是 stale 或 stopped，或某個 service 的健康 backend 少於 `-c` 個（預設 1：全部故障，或全部被 drain）。
- WARNING：健康的少於 `-w` 個（預設：每一個沒有被 drain 的 backend）。
- UNKNOWN：`emfor` 或 `jq` 無法執行、讀不到 store，或 service 不存在。
- 被 drain 的 backend 不計入（它是刻意被拿掉的），但會在文字裡點名。`-s` 選一個 service（預設全部），`-C` 指定寫著 store 的設定檔。
- 監控程式所用的使用者必須能讀 store、也能寫它的 lock 檔（[誰能使用 store](#誰能使用-store)）；採用建議的 `lock_file_mode = "0660"` 時，這個使用者必須在那個群組裡。用 NRPE 或 SSH 時，腳本在有 store 的那台主機上執行。

**把 Nagios 外掛當成檢查：`emfor-exec.sh`。** emfor 的 [`exec` 檢查](04-health-checks.md#exec)不經 shell 啟動程式，並把 backend 放在環境變數（`EMFOR_IP`、`EMFOR_PORT`），所以設定檔裡沒辦法給外掛 `-H $EMFOR_IP`。`emfor-exec.sh` 就是做這件事並執行外掛：

```toml
[services."web.svc.emfor"]
check = "exec"
backends = ["10.0.0.11:80", "10.0.0.12:80"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/emfor-exec.sh",
    "/usr/local/libexec/nagios/check_http", "-u", "/healthz"] }
```

它執行 `check_http -H <ip> -p <port> -u /healthz`，並回傳外掛的結束狀態；外掛用別的旗標時，用 `--host-flag` 與 `--port-flag` 換掉。emfor 怎麼看這個結果：0 是 up，**1（WARNING）是 down**，2（CRITICAL）是 down，3（UNKNOWN）是 error：狀態一開始不變，只有連續 error 超過 `fall` x `interval` 之後才變成 `unknown`（[第 4 章](04-health-checks.md#risefall-與-error)）；外掛輸出的第一行是 reason，在 `timeout` 內沒有結束的外掛是 down。`--warning up` 把 WARNING 當成 up。每個 backend 的每次檢查都會啟動一個行程，所以選 `interval` 時要把這點算進去（[第 4 章](04-health-checks.md#exec)）。

`tests/nagios/run.sh` 在 Docker 裡檢查這兩支腳本，外掛用一個替身，它依檔案內容說 ok、warn、crit、unknown 或 hang。沒試過：Monitoring Plugins 或 Nagios Plugins 套件裡的真正外掛、Nagios 或 Icinga 本體、NRPE、Zabbix（它的 `UserParameter` 可以執行 `check_emfor.sh` 並取用它的文字）、FreeBSD。

## 執行多個 daemon

每個 store 只有一個寫入者：daemon 鎖住 `<store>.writer`，使用相同 `[store] path` 的第二個 daemon 會以 1 結束。要在同一台主機上執行兩個 daemon（測試用），請給它們不同的 store 路徑、DNS port 與 metrics port。

## 升級

從同一個發行版安裝 daemon 與所有 client 函式庫，順序如下：

1. 升級主機上的 client 函式庫（以及 `emfor status`），並重新啟動應用程式讓它們載入新的函式庫：執行中的應用程式一直用它啟動時載入的那一版。比 daemon 舊的 client 可能每次查詢都以 `INCOMPATIBLE` 失敗（0.4.0 起 store 格式是版本 2，見 [store 格式版本](06-clients.md#store-格式版本)），而且較舊的 client 仍可能在 open service 的最後一層選到被 drain 的 backend（[drain 一個 backend](05-backend-selection.md#drain-一個-backend)）。
2. 重新啟動之前，先用新的 binary 執行 `emfor check-config`。0.5.0 會拒絕 0.4.0 接受的選項（高於 `24h` 的 `interval`、低於 `10ms` 的 `timeout` 等，見 [CHANGELOG.md](../../CHANGELOG.md)）。設定無效時，`systemctl restart emfor` 會讓 daemon 停著（啟動前的檢查失敗，unit 最後是 `failed`，要 `systemctl reset-failed emfor` 才能再啟動）：見 daemon 指南的 [Restarts](../deploy/daemon.md#running-under-systemd)。
3. 安裝 daemon 並重新啟動。

會改變做法的版本說明：0.4.0 寫入 store 格式版本 2，並預設加密 peer 訊息（聯防群組一台一台升級，升級時先設 `encrypt = false`，見[聯防群組](#聯防群組)）；0.5.0 更嚴格地驗證設定。

重新啟動之後，已經在用新函式庫的應用程式會跟上重新啟動後 daemon 的 store，不必再重新啟動。

### 重新啟動時應用程式會看到什麼

查詢從不等待 daemon：它一定從 store 得到答案。但重新啟動前後有一小段時間，store 分不出好的和壞的 backend：

- **daemon 停止期間。** 正常停止後 store 立刻是 stale（heartbeat 為 0）；被 kill 的話，client 會再相信舊狀態最多 3 秒，之後才視為 stale。stale 的 store 裡每個 backend 都是候選：`open` 服務回任何一個，`closed` 服務以 `ALL_DOWN` 失敗。
- **啟動後的最初 0.2 到 0.4 秒。** daemon 把每個 backend 標成 `unknown`，並大約每 200 ms 檢查一次。在第一批結果出來之前，它們全都是候選：`open` 回任何一個 backend，`closed` 在它的某個 backend 變成 healthy 之前以 `ALL_DOWN` 失敗。

實測方式是 daemon 立刻再啟動（SIGTERM 之後、SIGKILL 之後各測）：每種情況重複 20 次，一台主機（Debian 容器）、loopback、預設設定的 `tcp` 檢查、兩個服務各有四個 backend 且其中兩個是壞的，client 每毫秒查詢一次。最後一個錯誤答案出現在訊號後約 0.25 秒（中位數；最久約 0.4 秒）。在這段時間裡，`open` 的答案有一半是壞掉的 backend，每次 `closed` 查詢都以 `ALL_DOWN` 失敗。範例 systemd unit 每次啟動前會執行的 `emfor check-config` 約 1 ms。daemon 停了 2 秒的話，這段時間是 2.2 到 2.4 秒。backend 全部正常的服務不受影響，因為這時任何一個 backend 都是好答案。其他硬體、網路、interval 與檢查會得到不同的數字。

可以這樣處理：

- 主機在 load balancer 後面時，先把它移出 rotation，等進行中的請求結束再重新啟動，等 `emfor status` 的結束代碼為 0（daemon 正在執行，且顯示的每個服務都有健康的 backend；見 [emfor status](#emfor-status)）再放回去。不經過 load balancer 的工作（批次工作、queue worker、會重新連線的連線池）在重新啟動期間仍會查詢。
- 設定 `on_all_down = "closed"` 的服務、使用 `failover` 而第一個 backend 是壞的服務，以及有備援 backend 的服務，受影響最大；使用它們的主機請照上面的方式做。
- 先安裝新的套件，再重新啟動。不要「停止 daemon、安裝、再啟動」：那樣空窗就和安裝一樣長。
- 一次只重新啟動一台主機。
- 只改設定不需要重新啟動：用 reload，檢查沒有改變的 backend 會保留狀態（見 [reload 會改變什麼](03-configuration.md#reload-會改變什麼)）。

**從 0.1.x 升級到 0.2.0**：daemon 現在要用指令啟動。重新啟動前先修改服務定義：

| 0.1.x | 0.2.0 |
| --- | --- |
| `emfor --config X` | `emfor server --config X` |
| `emfor --check-config --config X` | `emfor check-config --config X` |

store 格式與 plugin 協定沒有改變。請以新的範例取代 0.1.x 版 systemd unit 與 rc.d script 的複本。所有變更見 [CHANGELOG.md](../../CHANGELOG.md)。
