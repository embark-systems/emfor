# 8. 維運

[手冊](README.md) · 上一章：[7. DNS](07-dns.md) · 下一章：[9. 疑難排解](09-troubleshooting.md)

## 指令

| 指令 | 作用 |
| --- | --- |
| `emfor server` | 在前景執行 daemon。 |
| `emfor check-config` | 驗證設定檔，有效時結束代碼為 0，無效時為 2。 |
| `emfor status` | 顯示執行中的 daemon 發布了什麼。 |
| `emfor --help` | 列出指令與預設設定檔路徑。 |

`--config <path>` 可放在指令之前或之後。不帶指令的 `emfor` 只印出用法並以 2 結束；它絕不會啟動 daemon。

## Signal

| Signal | 效果 |
| --- | --- |
| `SIGHUP` | 重新載入設定。只有整份檔案都有效時才套用；否則記錄錯誤並繼續使用舊設定。見 [reload 會改變什麼](03-configuration.md#reload-會改變什麼)。 |
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

`emfor status` 只讀取設定檔的 `[store]` table，並像 client 一樣以唯讀方式開啟 store；任何能讀取 store 的使用者都能執行它。

- **daemon**：heartbeat 不超過 3 秒時為 `running`，超過時為 `stale`（daemon 卡住或已死），正常停止後為 `stopped`。`stale` 或 `stopped` 時，顯示的服務只是 daemon 最後寫入的內容。
- **服務**：`balance`、`on_all_down`，沒有 backend 健康時顯示 `ALL DOWN`。
- **backend**：狀態、距上次檢查多久、平滑後的延遲、`fastest` 服務中快的 backend 標示 `pool`、10 分鐘內在 healthy 與 unhealthy 之間變化 5 次以上的 backend 標示 `FLAPPING (<n> changes in 10m)`，以及最近一次失敗檢查的 reason。

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
| `emfor_service_all_down` | 沒有健康的 backend 時為 1 |
| `emfor_backend_checks_total{result}` | 依結果分類的檢查次數：`up`、`down`、`error` |
| `emfor_backend_transitions_total{to}` | 依新狀態分類的狀態變化次數 |
| `emfor_store_commit_failures_total` | 失敗的 store commit 次數 |
| `emfor_store_last_commit_timestamp_seconds` | 上次成功 commit 的時間；應在幾秒以內 |
| `emfor_store_used_bytes`、`emfor_store_map_size_bytes` | store 使用量與 `[store] map_size` |
| `emfor_plugin_up`、`emfor_plugin_restarts_total` | plugin 健康狀況 |
| `emfor_peer_up{peer}` | 有 `[peers]` 時：該成員存活時為 1 |
| `emfor_peer_messages_rejected_total{reason}` | 有 `[peers]` 時：依原因分類的丟棄訊息數：`malformed`、`auth`（密鑰錯誤，或無法解密的訊息）、`sender`（不是成員，或偽造的位址）、`site`、`stale`（時鐘相差超過 5 秒）、`replay` |
| `emfor_peer_notifications_total{direction}` | 有 `[peers]` 時：故障通知 `sent`（每個成員算一次）與 `received` |
| `emfor_peer_results_total{direction}` | 有 `[peers]` 時：負責主機的檢查結果 `sent`（每個成員算一次）與 `received` |
| `emfor_peer_disagreements` | 有 `[peers]` 時：群組投票判為 down、而本機檢查通過的 backend 數 |
| `emfor_peer_checks_skipped_total` | 有 `[peers]` 時：因兩台負責主機意見一致而沒有做的檢查次數 |
| `emfor_build_info{version}`、`emfor_start_time_seconds` | daemon 本身 |

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml) 提供告警規則：endpoint 無法抓取、服務 `ALL DOWN`、store commit 失敗、backend 不健康達 10 分鐘、flapping、大多是 error，或不再被檢查。完整的 metrics 與規則清單見 [docs/deploy/daemon.md](../deploy/daemon.md#metrics)。

### 聯防群組

聯防群組的成員之間需要能以 UDP 連到彼此的 `[peers] listen` port（範例中為 5302），時鐘相差不能超過 5 秒（NTP）。某個成員的 `emfor_peer_up` 一直是 0 時，看兩邊的 `emfor_peer_messages_rejected_total`：`auth` 表示密鑰不同，`stale` 表示時鐘偏移，`sender` 表示 `members` 或 `listen` 不符，`site` 表示兩台主機的 `site` 不同。0.3.0 成員那邊的 `malformed` 表示它讀不了已升級成員的加密訊息：在它升級前，已升級的成員先設 `encrypt = false`（見 [`[peers]`](03-configuration.md#peers)）。警告 `peer <address> sends plaintext messages (EMP1); set encrypt = true there` 表示該成員仍送未加密的訊息；這些訊息照常接受，除非設了 `accept_plaintext = false`：那時它們算成 `plaintext`，該成員在這裡不算存活。

原因會說明 `down` 從哪裡來：`peers 2/3: <原因>` 表示三台負責主機中有兩台看到 backend down（原因是它們的）；`local: <原因>` 表示只有這台主機看到 down，投票沒有判 down；沒有前綴的原因表示沒有投票（存活成員少於三台，或沒有夠新的結果）。`emfor status` 在 daemon 那一行下面顯示群組狀態：

```
peers: 1/2 live, not live 10.0.0.7:5302
peers: check definition of api.svc.emfor differs at 10.0.0.6:5302
```

群組投票判為 down、而本機檢查通過的 backend 會標上 `local-up`。這些值得看一下：這台主機連得到其他主機連不到的 backend（常見原因是網路分割，或其他主機上的防火牆規則），`emfor_peer_disagreements` 會計算它們的數量。「check definition differs」那一行表示該成員這個服務的 `check`、`check_options` 或 `timeout` 和這台主機不同，所以它的結果不計入；請讓設定一致。同樣的資料也在 store 的 `meta/peers` key，以及 `emfor status --json` 的 `peers` 中。其中 `services_off` 列出這台主機不參加群組的服務（`peers = false`）；它們的 backend 不會標 `voted`，也沒有 `assigned_checkers`。`members` 的每一項也有自己的 `services_off`：該成員宣告不參加、而這台主機也有的服務。這台主機挑這些服務的負責主機時不算那個成員，`emfor status` 也照同樣方式排除。文字輸出會在 `peers:` 那一行之後，為每個這樣的成員加一行，只列出目前顯示的服務：

```
peers: 10.0.0.6:5302 leaves out api.svc.emfor db.svc.emfor
```

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

daemon 沒有執行或沒有 `meta/peers` 時它是 `null`。status 依 `meta/peers` 中的存活成員算出這些內容，而 daemon 每秒更新一次 `meta/peers`，所以成員剛加入或離開時最多會落後一秒。`voted` 只表示這台主機不是負責主機；某次檢查時間是否由投票決定，要看當下的票。`emfor_backend_checks_total` 仍然計算每一次檢查時間，不論是否由投票決定。backend 的延遲只由這台主機自己的檢查量測，所以在這種主機上大約每三個 `interval` 才變一次。

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
| 2 | 設定無效，或沒有給指令 | 沒用；修正設定 |
| 70 | state writer 或 DNS listener 在執行中停止了 | 有用；systemd unit 會重新啟動它 |

## 磁碟上的 store

daemon 至少每秒 commit 一次 store，每次狀態變化也會 commit。預設（`[store] sync = false`）commit 不等待磁碟，所以磁碟卡住時不會讓 heartbeat 延遲、使主機上所有應用程式都看到過期的 store。store 中沒有任何東西需要在主機當機後保留：daemon 啟動時會重寫每個服務。只有狀態歷史（`emfor status --history`）會跨重新啟動保留。

斷電後 store 可能損毀。這時 daemon 會記錄警告、把它移到 `<store>.corrupt`，並以新的 store 啟動；執行中的應用程式會自行跟上新檔案。唯一遺失的是狀態歷史，也就是 `emfor status --history` 顯示的每個 backend 最近 20 筆狀態變化。每次狀態變化也都記在 daemon 的 log 裡（`changed from healthy to unhealthy` 等），要查當機前發生了什麼，請看 log。設定 `sync = true` 會讓每次 commit 都寫入磁碟，但要承擔上述的風險。

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

## 執行多個 daemon

每個 store 只有一個寫入者：daemon 鎖住 `<store>.writer`，使用相同 `[store] path` 的第二個 daemon 會以 1 結束。要在同一台主機上執行兩個 daemon（測試用），請給它們不同的 store 路徑、DNS port 與 metrics port。

## 升級

從同一個發行版安裝 daemon 與所有 client 函式庫，然後重新啟動 daemon。應用程式不必停止；它們會跟上重新啟動後 daemon 的 store，不需要自行重新啟動。

**從 0.1.x 升級到 0.2.0**：daemon 現在要用指令啟動。重新啟動前先修改服務定義：

| 0.1.x | 0.2.0 |
| --- | --- |
| `emfor --config X` | `emfor server --config X` |
| `emfor --check-config --config X` | `emfor check-config --config X` |

store 格式與 plugin 協定沒有改變。請以新的範例取代 0.1.x 版 systemd unit 與 rc.d script 的複本。所有變更見 [CHANGELOG.md](../../CHANGELOG.md)。
