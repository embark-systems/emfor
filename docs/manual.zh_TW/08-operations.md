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
| `--watch` | 每秒重繪、標示變化，並列出最近 10 次變化。 |
| `--history` | 列出每個 backend 最近 20 次狀態變化（重新啟動後仍保留）。 |

| 結束代碼 | 何時 |
| --- | --- |
| 0 | daemon 正在執行，且顯示的每個服務都有健康的 backend。 |
| 1 | daemon 為 stale 或 stopped，或顯示的某個服務為 `ALL DOWN`。 |
| 2 | 無法讀取設定檔或 store、store 格式版本不同，或 `--service` 指定的服務不存在。 |

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
| `emfor_build_info{version}`、`emfor_start_time_seconds` | daemon 本身 |

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml) 提供告警規則：endpoint 無法抓取、服務 `ALL DOWN`、store commit 失敗、backend 不健康達 10 分鐘、flapping、大多是 error，或不再被檢查。完整的 metrics 與規則清單見 [docs/deploy/daemon.md](../deploy/daemon.md#metrics)。

## Log

daemon 把 log 寫到 stderr：systemd 下進 journal，FreeBSD rc.d script 下進 syslog（tag `emfor`）。以 `[log] level` 設定等級，或用 `RUST_LOG`（例如 `RUST_LOG=debug`）只為某次執行設定。只有 stderr 是終端機且未設定 `NO_COLOR` 時才會使用顏色。

| 等級 | 看到什麼 |
| --- | --- |
| `error` | store commit 失敗、被拒絕的 reload |
| `warn` | plugin 結束、損毀的 store 被替換、在 DNS zone 之外或 port 不一致的服務、無法解析的 plugin 輸出 |
| `info` | 啟動與停止、每次 backend 狀態變化與其 reason、plugin 的 stderr 內容 |
| `debug` | 每次通過的檢查所花的時間 |

## 結束代碼

| 代碼 | 意義 | 重試有用嗎？ |
| --- | --- | --- |
| 0 | 正常停止（`SIGTERM` / `SIGINT`） | — |
| 1 | 無法啟動：listen 位址已被使用、無法開啟 store，或另一個 emfor 持有這個 store（`<store>.writer`） | 沒用；先排除原因 |
| 2 | 設定無效，或沒有給指令 | 沒用；修正設定 |
| 70 | state writer 停止了（程式錯誤） | 有用；systemd unit 會重新啟動它 |

## 磁碟上的 store

daemon 至少每秒 commit 一次 store，每次狀態變化也會 commit。預設（`[store] sync = false`）commit 不等待磁碟，所以磁碟卡住時不會讓 heartbeat 延遲、使主機上所有應用程式都看到過期的 store。store 中沒有任何東西需要在主機當機後保留：daemon 啟動時會重寫每個服務。只有狀態歷史（`emfor status --history`）會跨重新啟動保留。

斷電後 store 可能損毀。這時 daemon 會記錄警告、把它移到 `<store>.corrupt`，並以新的 store 啟動；執行中的應用程式會自行跟上新檔案。設定 `sync = true` 會讓每次 commit 都寫入磁碟，但要承擔上述的風險。

commit 在自己的執行緒上執行，所以慢的磁碟不會拖慢 DNS 回答、metrics 或檢查。請留意 `emfor_store_used_bytes` 與 `map_size` 的比例：store 滿了之後 commit 會失敗，client 很快就會看到過期的 store。

## 誰能使用 store

應用程式讀取資料檔，並在 lock 檔 `<store>-lock` 中登記讀取，所以每個應用程式使用者都需要 store 的讀取權限與 lock 檔的寫入權限。預設值（`file_mode = "0644"`、`lock_file_mode = "0666"`）讓每個本機使用者都能這麼做，但也讓任何能寫 lock 檔的本機使用者可以卡住 daemon 的 commit 或干擾查詢。只有在每個本機使用者都可信任的主機上才保留預設值。否則，為你的應用程式建立一個群組：

1. 建立一個群組，例如 `emfor-clients`，把每個應用程式使用者加進去。
2. 把它設為 daemon 的群組。systemd 下：`systemctl edit emfor`，加上 `Group=emfor-clients` 並設定 `StateDirectoryMode=0750`。FreeBSD 下：`chgrp emfor-clients /var/lib/emfor && chmod 0750 /var/lib/emfor`。
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
