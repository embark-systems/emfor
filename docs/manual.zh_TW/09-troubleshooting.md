# 9. 疑難排解

[手冊](README.md) · 上一章：[8. 維運](08-operations.md)

先執行 `emfor status`：它顯示 daemon 是否在執行、每個 backend 的狀態，以及最近一次失敗檢查的 reason。接著看 daemon 的 log（`journalctl -u emfor`，FreeBSD 上看 syslog）。

## daemon 無法啟動

| 訊息或結束代碼 | 原因 | 處理 |
| --- | --- | --- |
| 結束代碼 2，列出設定錯誤 | 設定無效。 | `emfor check-config` 會列出每個錯誤與其 TOML 路徑。 |
| 結束代碼 2，印出用法 | 沒有給指令（0.1.x 的用法）。 | 使用 `emfor server`。 |
| `--check-config was replaced by "emfor check-config"` | 0.1.x 的命令列。 | 見[升級](08-operations.md#升級)。 |
| 結束代碼 1，指出某個位址 | `[dns]` 或 `[metrics]` 的 listen 位址已被使用，或需要權限（53 port）。 | 釋放該 port，或改用其他 port。 |
| 結束代碼 1，指出 `<store>.writer` | 已有另一個 emfor 使用這個 store。 | 停止它，或改用其他 `[store] path`。 |
| 結束代碼 1，無法開啟 store | 目錄不存在、daemon 的使用者無法寫入，或 lock 檔屬於其他使用者。 | 以正確的擁有者建立目錄；見 [lock 檔屬於其他使用者](#lock-檔屬於其他使用者)。 |
| `plugins."x".command: ...` | plugin 無法啟動或沒有完成 handshake。 | 手動執行該指令；檢查路徑與權限。 |

## 每次查詢都失敗

| 錯誤 | 原因 | 處理 |
| --- | --- | --- |
| `UNAVAILABLE`，檔案不存在 | daemon 從未執行過，或 `EMFOR_STORE` 與 `[store] path` 不同。 | 啟動 daemon；設定 `EMFOR_STORE`。 |
| `UNAVAILABLE`，permission denied | 應用程式使用者無法讀取 store 或無法寫入 `<store>-lock`。 | 見[誰能使用 store](08-operations.md#誰能使用-store)。 |
| `UNAVAILABLE`，提到 LMDB 格式或發行版 | `libemfor` 與 daemon 來自不同的 emfor 發行版。 | 兩者從同一發行版安裝。 |
| `UNAVAILABLE`，提到 `EMFOR_MAX_STALENESS` | 這個變數的值無效。 | 使用整數加上 `ms` 或 `s`。 |
| `INCOMPATIBLE` | store 的格式版本不同。 | 安裝與 daemon 同一發行版的 client。 |
| `NOT_FOUND` | 執行中的設定裡沒有這個名稱的服務。 | 檢查拼字，並確認 reload 已套用（`emfor status`）。 |

## 某個服務 ALL DOWN

1. 用 `emfor status --service <name>` 看 reason。
2. 如果每個 backend 都是同樣看起來像本機問題的 reason（TLS、CA 檔、`connect: Network is unreachable`），問題多半在檢查端主機，而不是 backend。
3. 如果 daemon 那一行顯示 `stale` 或 `stopped`，壞的不是 backend，而是 daemon。在它恢復執行之前，每個 `closed` 的服務都會失敗。重新啟動它並查看 log。
4. 檢查 check 本身：從這台主機用 `curl` 或 `nc`，以相同的路徑、port 與選項連到 backend 試試。

## backend 反覆變化（flapping）

`emfor status` 中的 `FLAPPING` 或 `EmforBackendFlapping` 告警，表示在 healthy 與 unhealthy 之間頻繁變化。常見原因：`timeout` 太接近 backend 平常的回應時間，或 backend 過載。調高 `timeout`（最多到 `interval`）、調高 `fall` 與 `rise`，或檢查 backend。`emfor status --history --service <name>` 會顯示何時、為何變化。

## 檢查結果是 error

`error` 不改變 backend 的狀態，但 backend 只收到 error 達 `fall × interval` 時會回到 `unknown`。常見的 reason：

| reason 開頭 | 原因 |
| --- | --- |
| `plugin <name> unavailable`、`plugin exited` | plugin 當掉、不再回應，或正在重新啟動；從 log 看它的結束狀態或 `stopped answering`。 |
| `plugin did not answer within timeout` | plugin 太慢，或一次只處理一個檢查。 |
| `password_file: `、`ca_file: ` | daemon 的使用者無法讀取該檔案。 |
| `exit code N`（exec，N 不是 1 或 2） | script 以預期外的方式失敗。 |

## DNS 無法解析

| 症狀 | 原因 | 處理 |
| --- | --- | --- |
| `dig @127.0.0.1 -p 5300` 沒有回應 | 沒有 `[dns]` table，或 listen 位址不同。 | 看 `emfor status`（daemon 那一行會顯示 `dns ...`）。 |
| `REFUSED` | 名稱在 zone 之外。 | 把服務命名在 zone 之下，或修改 `zone`。 |
| `NXDOMAIN` | 沒有這個名稱的服務。 | 檢查名稱。 |
| `SERVFAIL` | 沒有健康的 backend 且 `on_all_down = "closed"`；或會驗證的 unbound 沒有設定 `domain-insecure`。 | 見[某個服務 ALL DOWN](#某個服務-all-down)；修正 unbound 設定。 |
| 直接查詢可以，`getent hosts` 不行 | 主機的 resolver 沒有轉送這個 zone。 | 照[第 7 章](07-dns.md#從主機的-resolver-轉送)設定。 |
| 故障的 backend 仍被回傳 | resolver 或應用程式的快取。 | 見[快取](07-dns.md#快取)。 |

## daemon 在執行，store 卻看起來過期

`emfor status` 顯示 `stale`，或 client 的行為像是所有 backend 都 down 了，但 daemon 行程還活著：

- `emfor_store_commit_failures_total` 持續增加，log 中出現 `MDB_MAP_FULL`：store 滿了。調高 `[store] map_size` 並重新啟動。
- commit 因其他原因失敗：log 會說明。
- 系統時鐘往前跳：在下一次 commit（約一秒後）之前，heartbeat 看起來會是舊的。
- 有本機使用者持有 store 的寫入鎖（lock 檔可被寫入）；見[誰能使用 store](08-operations.md#誰能使用-store)。

## lock 檔屬於其他使用者

如果 lock 檔 `<store>-lock` 在應用程式執行期間被刪除，第一個開啟 store 的行程會以自己的身分重建它，而以另一個使用者執行的 daemon 可能就無法開啟 store。先停止應用程式，或修正該檔案的擁有者與權限（`chown emfor`、`chmod` 成 `lock_file_mode`），再啟動 daemon。絕不要單獨刪除 lock 檔。

## reload 沒有生效

reload 是全有或全無。在 log 中找 `reload rejected` 與錯誤訊息，或執行 `emfor check-config`。`[store]`、`[dns] listen` 與 `[metrics] listen` 的變更一定要重新啟動。
