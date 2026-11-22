# 1. emfor 是什麼，不是什麼

[手冊](README.md) · 下一章：[2. 快速上手](02-quick-start.md)

## 問題

你的應用程式要和其他服務溝通：一個有 primary 與 replica 的資料庫、一組 API 伺服器、一個快取、一台 LDAP 或 RADIUS 伺服器。每個服務都有好幾個位址，而且不時會有一個掛掉。應用程式得在連線之前先知道哪個位址能用。

常見的解法是在中間放一台負載平衡器（多一跳，也多一個會壞的東西），或是用一套有中央叢集的服務註冊中心（多一套要維運的系統）。emfor 的做法不一樣。

## emfor 給的答案

emfor 是一個小型 daemon，**每一台應用程式主機上**都跑一個。每個 daemon 會：

1. 從自己的設定檔讀出服務清單，以及每個服務的 backend（IP 位址與 port）；
2. 用你選的檢查方式（TCP 連線、HTTP 請求、資料庫登入、script）檢查每個 backend，預設每兩秒一次；
3. 把結果寫進 **state store**，也就是同一台主機上的一個本機檔案（LMDB 資料庫）。

應用程式需要 backend 時，就呼叫該語言的 emfor **client 函式庫**：`lookup("db.svc.emfor")`。函式庫讀本機的 store，回傳一個健康的位址，例如 `10.0.3.1:5432`。整個過程不走網路，一次查詢大約一微秒，所以應用程式每次建立新連線前都可以先問一次。

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

沒辦法用 client 函式庫的程式，可以改問 emfor 的 DNS server（選用，提供 A、AAAA 與 SRV 記錄），或在 shell script 裡跑 `emfor lookup <service>`。

## 它還做什麼

- **在健康的 backend 裡挑一個**：round-robin、sticky（依你傳入的 key）、failover、weighted、依資料中心（locality），或 fastest。
- **備援、drain 與慢啟動**：把某個 backend 留到其他的都失敗時才用；把某個 backend 拿出來維護，檢查照跑；或讓剛恢復的 backend 慢慢增加流量。
- **聯防群組**：同一個資料中心的 daemon 可以分享檢查結果。某一台看到故障，所有成員會立刻重新檢查，再用多數決決定。
- **Plugin**：`emfor-check` 能檢查 redis、MySQL/MariaDB、PostgreSQL、LDAP、DNS、RADIUS、SIP、Diameter、SMTP、AMQP、MongoDB、gRPC 與 QUIC/HTTP/3。你也可以自己寫 plugin，或把任何 script 當成 `exec` 檢查來跑。
- **維運**：`emfor status`、`emfor lookup --explain`、`emfor check-config`、Prometheus metrics、套件（deb、rpm），以及一個範例 FreeBSD port。

## 出問題時會發生什麼事

| 什麼故障 | 應用程式拿到什麼 |
| --- | --- |
| 一個 backend | 3 次檢查失敗後（用預設值大約 6 秒），就不會再回傳它。只要還有別的 backend 通過檢查，第一次檢查失敗時查詢就已經改到其他 backend。 |
| 某服務的所有 backend | 預設（`on_all_down = "open"`）還是會拿到一個位址，因為有可能是檢查本身出錯；設成 `on_all_down = "closed"` 時則拿到錯誤 `ALL_DOWN`。 |
| 這台主機上的 emfor daemon | 3 秒後 store 就算**過期**（stale）：函式庫不再相信它，當成沒有任何 backend 健康來處理（`open` 的服務還是會回傳一個位址）。查詢永遠不會卡住。 |
| 這台主機的網路 | 檢查會失敗，反正這台主機上的應用程式本來也連不到那些 backend。 |
| 另一台主機，或中央伺服器 | 沒有影響：沒有中央伺服器，每台主機各自判斷。 |

## emfor 不是什麼

emfor **不在資料路徑上**。它從來不經手任何請求或連線。所以它又快又簡單，但也代表 proxy 對流量做的那些事，它都做不到：

- 沒有 least-connections 平衡、沒有連線數上限、沒有佇列；
- 不會重試失敗的請求（由應用程式自己重試，下一次查詢就會拿到另一個 backend）；
- 沒有被動健康檢查（因為實際請求失敗而把伺服器標成 down），只有主動檢查；
- 不做 TLS 終結，也不依 URL、header 或 cookie 路由。

emfor 也**不是服務註冊中心**。backend 不會自己註冊。backend 清單寫在每台主機的設定檔裡，由你或你的設定管理工具寫進去。它沒有 key-value store，也沒有中央的全貌；每台主機只有自己看到的那一份（想看全部，請用 Prometheus）。

如果你需要 proxy 的功能，就繼續用 proxy。emfor 還是可以幫 proxy 決定要用哪些 backend：見[第 14 章](14-integrations.md)。

## 什麼時候適合用 emfor

- backend 是一份已知的 IP 位址清單：虛擬機器、實體機、資料庫、AAA 伺服器。
- 你希望每台應用程式主機從自己的角度判斷，不必依賴一個所有主機都靠它的中央元件。
- 應用程式可以在連線前先問 emfor：透過 client 函式庫（Rust、C、Go、Python、Perl、PHP）、透過 DNS，或透過 `emfor lookup`。

## 平台

Linux 是主要平台，套件格式是 deb 與 rpm。FreeBSD 透過一個範例 port 支援，已經在 FreeBSD 15.1（arm64，在虛擬機器裡）上跑過；FreeBSD 13.x 與 14.x，以及專案 CI job 以外的 amd64，都還沒有手動測試過。macOS 只拿來開發。

下一章：[2. 快速上手](02-quick-start.md)
