# 1. emfor 是什麼，不是什麼

[手冊](README.md) · 下一章：[2. 快速上手](02-quick-start.md)

## 問題

你的應用程式會和其他服務溝通：有一台 primary 與一台 replica 的資料庫、一組 API 伺服器、一個快取、一台 LDAP 或 RADIUS 伺服器。每個服務都有好幾個位址，而且不時會有其中一個掛掉。應用程式需要在連線之前知道哪個位址能用。

常見的做法是在中間放一台負載平衡器（多一跳，也多一個可能故障的東西），或是用一個有中央叢集的服務註冊中心（多一套要維運的系統）。emfor 的做法不同。

## emfor 給的答案

emfor 是一個小型 daemon，**在每一台應用程式主機上**執行。每個 daemon：

1. 從自己的設定檔讀取服務清單與它們的 backend（IP 位址與 port）；
2. 用你選的檢查（TCP 連線、HTTP 請求、資料庫登入、script）檢查每個 backend，預設每兩秒一次；
3. 把結果寫進 **state store**：同一台主機上的一個本機檔案（LMDB 資料庫）。

應用程式需要 backend 時，呼叫所用語言的 emfor **client 函式庫**：`lookup("db.svc.emfor")`。函式庫讀取本機的 store，回傳一個健康的位址，例如 `10.0.3.1:5432`。不經過網路，一次查詢大約一微秒，所以應用程式可以在每次建立新連線之前都問一次。

```
                    checks (tcp, http, plugins, scripts)
     backends  <------------------------------------------  emfor daemon
   10.0.3.1:5432                                                |  writes
   10.0.3.2:5432                                                v
                                                   /var/lib/emfor/state.mdb
                                                                |  reads (about 1 µs)
                                                                v
                                     application + client library  ---->  10.0.3.1:5432
```

無法使用 client 函式庫的程式，可以改問 emfor 選用的 DNS server（A、AAAA 與 SRV 記錄），或在 shell script 裡執行 `emfor lookup <service>`。

## 它還做什麼

- **在健康的 backend 之間挑選**：round-robin、sticky（依你傳入的 key）、failover、weighted、依資料中心（locality），或 fastest。
- **備援、drain 與慢啟動**：把一個 backend 保留到其他的都失敗時才用、在檢查照跑的同時把一個拿出來維護，或讓剛恢復的 backend 逐步增加流量。
- **聯防群組**：同一個資料中心的 daemon 可以分享檢查結果，所以一台看到的故障會由所有成員立刻重新檢查，並以多數決決定。
- **Plugin**：`emfor-check` 可檢查 redis、MySQL/MariaDB、PostgreSQL、LDAP、DNS、RADIUS、SIP、Diameter、SMTP、AMQP、MongoDB、gRPC 與 QUIC/HTTP/3。你可以自己寫 plugin，或把任何 script 當作 `exec` 檢查執行。
- **維運**：`emfor status`、`emfor lookup --explain`、`emfor check-config`、Prometheus metrics、套件（deb、rpm），以及一個範例 FreeBSD port。

## 出問題時會發生什麼事

| 什麼故障 | 應用程式拿到什麼 |
| --- | --- |
| 一個 backend | 3 次檢查失敗後（預設值下約 6 秒）不再被回傳。只要還有其他 backend 通過檢查，第一次檢查失敗時查詢就已經改到其他 backend。 |
| 某服務的所有 backend | 預設（`on_all_down = "open"`）仍拿到一個位址，因為可能是檢查出錯；或者在 `on_all_down = "closed"` 時拿到錯誤 `ALL_DOWN`。 |
| 這台主機上的 emfor daemon | 3 秒後 store 算是**過期**（stale）：函式庫不再相信它，當作沒有任何 backend 健康來處理（`open` 的服務仍回傳一個位址）。查詢永遠不會卡住。 |
| 這台主機的網路 | 檢查失敗，而這台主機上的應用程式本來也連不到那些 backend。 |
| 另一台主機，或中央伺服器 | 沒有影響：沒有中央伺服器，每台主機各自決定。 |

## emfor 不是什麼

emfor **不在資料路徑上**。它從不經手任何請求或連線。這讓它快速又簡單，也代表它做不到 proxy 對流量所做的事：

- 沒有 least-connections 平衡、沒有連線數限制、沒有佇列；
- 不重試失敗的請求（由應用程式重試，它的下一次查詢會拿到另一個 backend）；
- 沒有被動健康檢查（因為實際請求失敗而把伺服器標成 down）：只有主動檢查；
- 沒有 TLS 終結、不依 URL、header 或 cookie 路由。

emfor 也**不是服務註冊中心**。backend 不會自己註冊。backend 清單寫在每台主機的設定檔裡，由你或你的設定管理工具寫入。它沒有 key-value store，也沒有中央的全貌；每台主機有自己的一份（要看全部，請用 Prometheus）。

如果你需要 proxy 的功能，就留著 proxy。emfor 仍然可以決定 proxy 使用哪些 backend：見[第 14 章](14-integrations.md)。

## 什麼時候適合用 emfor

- backend 是一份已知的 IP 位址清單：虛擬機器、實體機、資料庫、AAA 伺服器。
- 你希望每台應用程式主機從自己的位置判斷，不依賴一個所有主機都要靠它的中央元件。
- 應用程式可以在連線前問 emfor：透過 client 函式庫（Rust、C、Go、Python、Perl、PHP）、透過 DNS，或透過 `emfor lookup`。

## 平台

Linux 是主要平台；套件是 deb 與 rpm。FreeBSD 透過一個範例 port 支援，它已在 FreeBSD 15.1（arm64，在虛擬機器中）上跑過；FreeBSD 13.x 與 14.x，以及專案 CI job 以外的 amd64，都還沒有手動測試過。macOS 只用於開發。

下一章：[2. 快速上手](02-quick-start.md)
