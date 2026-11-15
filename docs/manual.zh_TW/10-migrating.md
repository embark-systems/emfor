# 10. 從 HAProxy 或 Consul 搬過來

[手冊](README.md) · 上一章：[9. 疑難排解](09-troubleshooting.md)

這一章寫給已經用 HAProxy 或 Consul 讓應用程式避開壞掉的 backend、現在想知道同樣的工作在 emfor 裡長什麼樣子的人。它把兩者各一份看起來真實的設定翻成 `emfor.toml`，說明哪些東西沒有對應，最後介紹一個不碰流量、與現有方案並排試用 emfor 的做法。

請先讀[第 1 章](01-concepts.md)：整個架構的樣貌差異很大，只靠逐行翻譯會誤導你。

## emfor 適合取代嗎？

HAProxy 與 Consul 做的事裡，emfor 只做一件：從應用程式所在的主機檢查一份固定的 backend 清單，並告訴應用程式該用哪一台。它不在流量的路徑上，也沒有中央伺服器。

| | HAProxy | Consul | emfor |
| --- | --- | --- | --- |
| 流量走哪裡 | 經過 HAProxy（多一跳） | 直接到 backend | 直接到 backend |
| 誰決定用哪個 backend | HAProxy，每條連線都由它決定 | 應用程式，依 DNS 的回答 | 應用程式，依本機 store（client 函式庫或 `emfor lookup`）或 DNS 的回答 |
| 檢查在哪裡跑 | 在 HAProxy 裡 | 在執行該服務的節點上的 Consul agent | 在每台應用程式主機上，對每個 backend 檢查 |
| 有哪些 backend | 設定檔 | 服務自己註冊 | 每台主機的設定檔 |
| 中央伺服器 | 沒有 | 有（要有 quorum 的 server） | 沒有 |

所以 emfor 適合的情況是：backend 是已知的一份位址清單（虛擬機、實體機、資料庫、AAA server），而且應用程式能問 emfor 該連哪裡：用 client 函式庫、用 shell 指令，或透過 DNS。

它取代不了 proxy 對流量本身做的事：最少連線、重試、排隊、TLS 終結、依 URL 或 header 路由、連線數限制，以及被動健康檢查（因為真實請求失敗而把 server 標成 down）。如果你需要這些，請保留 HAProxy，並看[沒有對應的東西](#沒有對應的東西)。

## 從 HAProxy 搬過來

像這樣的 HAProxy backend：

```
backend api
    balance roundrobin
    option httpchk GET /healthz
    http-check expect status 200
    default-server inter 2s fall 3 rise 2
    server api1 10.0.0.11:8080 check
    server api2 10.0.0.12:8080 check weight 3
    server api3 10.0.1.11:8080 check backup slowstart 30s
```

在 `emfor.toml` 裡會變成這個服務：

```toml
[services."api.svc.emfor"]
check = "http"
balance = "weighted"
interval = "2s"
fall = 3
rise = 2
slow_start = "30s"
backends = [
  "10.0.0.11:8080",
  { addr = "10.0.0.12:8080", weight = 3 },
  { addr = "10.0.1.11:8080", backup = true },
]

[services."api.svc.emfor".check_options]
path = "/healthz"
expect_status = [200]
```

逐行對照：

| HAProxy | emfor | 說明 |
| --- | --- | --- |
| `backend api` | `[services."api.svc.emfor"]` | 服務名稱是類似主機名稱的字串，同時也是 DNS 名稱（[第 3 章](03-configuration.md#servicesname)）。 |
| `server api1 10.0.0.11:8080` | `backends` 的一個項目 | 只能寫 IP 位址，不能寫主機名稱：emfor 不依賴 DNS 去找它的 backend。 |
| `check` | `check = "http"`（或 `"tcp"`） | 整個服務共用一個 `check`。 |
| `inter 2s` | `interval = "2s"` | 預設都是 2 秒。 |
| `fall 3` / `rise 2` | `fall = 3` / `rise = 2` | 預設分別都是 3 與 2。 |
| `option httpchk GET /healthz` | `path = "/healthz"` | emfor 一律送 `GET`。HAProxy 不帶參數時送 `OPTIONS /`；emfor 不設 `path` 就送 `GET /`（[http 檢查](04-health-checks.md#http)）。 |
| `http-check expect status 200` | `expect_status = [200]` | 沒有設的話，200 到 399 的狀態碼都算 `up`；HAProxy 的預設也接受 2xx 與 3xx。範圍寫成 `"200-299"`。 |
| `http-check send ... hdr Host x` | `host_header = "x"` | 只能設 `Host` header。 |
| `weight 3` | `{ addr = "...", weight = 3 }` | 1 到 100，而且只有 `balance = "weighted"`（以及有 key 的 `sticky`）會用到。 |
| `backup` | `backup = true` | 只有在沒有其他 backend 通過檢查時才會用到備援（[備援 backend](05-backend-selection.md#備援-backend)）。 |
| `slowstart 30s` | `slow_start = "30s"` | 恢復的 backend 會分到逐漸增加的流量，分十個階段（[慢啟動](05-backend-selection.md#慢啟動slow-start)）。設在服務上，不是設在單一 backend。 |
| `balance roundrobin` | `balance = "round-robin"` 或 `"weighted"` | HAProxy 的 round robin 會考慮權重；emfor 的 `round-robin` 不會。用了權重就要用 `weighted`。 |
| `balance first` | `balance = "failover"` | 依設定順序取第一台健康的 backend。沒設 `maxconn` 時 HAProxy 的 `first` 做的事相同；設了的話，第一台滿了它就換下一台，emfor 沒有連線數限制。 |
| `balance source`、`balance uri`、`balance hdr` | `balance = "sticky"` | key 由應用程式提供：`lookup(name, key)`。同一個 key 在 backend 健康時會得到同一台（[第 5 章](05-backend-selection.md)）。 |
| `balance leastconn` | 無 | emfor 看不到連線數；見下面。 |
| `check port 8081` | 無 | emfor 檢查的是 backend 自己的位址與 port。 |
| `agent-check` | plugin 或 `exec` 檢查 | 沒有 agent 協定；把檢查寫成 [plugin 或 `exec` 檢查](04-health-checks.md#exec)。 |
| `maxconn`、排隊、`observe layer7` | 無 | 見下面。 |

接著，應用程式改成連 emfor 告訴它的位址，而不是連 HAProxy。挑適合它的方式：

```console
$ emfor lookup api.svc.emfor
10.0.0.11:8080
$ curl "http://$(emfor lookup api.svc.emfor)/healthz"
```

或是用它那個語言的 client 函式庫（[第 6 章](06-clients.md)），或是 DNS 名稱 `api.svc.emfor`，由主機的 resolver 轉送（[第 7 章](07-dns.md)）。用 `emfor lookup` 或 DNS 的話，應用程式除了連線的位址之外不需要改動。

### 與 HAProxy 並排試用

emfor 不碰流量，所以可以先在 HAProxy 旁邊跑：

1. 把服務寫進 `emfor.toml`，用 `emfor check-config` 檢查。
2. 啟動 daemon，把 `emfor status` 與 HAProxy 的 stats 頁面比對：健康與不健康的 backend 應該相同，`emfor status` 裡的 reason 會說明檢查為什麼失敗。
3. 用 `emfor lookup api.svc.emfor --explain` 看應用程式會拿到什麼、為什麼（[第 6 章](06-clients.md#從-shell-查詢emfor-lookup)）。
4. 一次只把一個應用程式搬到 emfor，同時看 [metrics](08-operations.md#metrics) 與 `emfor status --watch`。
5. 等到沒有任何應用程式再使用某個 backend 時，才把它從 HAProxy 的設定移除。

## 從 Consul 搬過來

一個帶 HTTP 檢查的 Consul 服務，由執行它的節點上的 agent 註冊：

```hcl
service {
  name = "api"
  port = 8080
  check {
    http                     = "http://localhost:8080/healthz"
    interval                 = "10s"
    timeout                  = "1s"
    success_before_passing   = 2
    failures_before_critical = 3
  }
}
```

同樣的工作在 emfor 裡，是一個列出各個節點的服務，由提出查詢的那台主機去檢查它們：

```toml
[services."api.service.consul"]
check = "http"
interval = "10s"
timeout = "1s"
rise = 2
fall = 3
backends = ["10.0.0.11:8080", "10.0.0.12:8080", "10.0.1.11:8080"]

[services."api.service.consul".check_options]
path = "/healthz"
expect_status = ["200-299"]

[dns]
zone = "service.consul"
```

服務叫 `api.service.consul`，zone 是 `service.consul`，所以只要主機的 resolver 把那個 zone 轉送給 emfor 而不是 Consul（[第 7 章](07-dns.md)），查 `api.service.consul` 的程式就照舊能用。（你也可以保留 emfor 自己的 zone `svc.emfor`，改用新的名稱。）

| Consul | emfor | 說明 |
| --- | --- | --- |
| `service { name, port }` | 一個服務與它的 `backends` | 沒有註冊這回事：backend 清單寫在每台主機的設定檔裡，由安裝設定的工具（Ansible、Puppet、範本）產生。新增節點就是改檔案再 reload（[reload 會改變什麼](03-configuration.md#reload-會改變什麼)）。 |
| `check { http = ... }` | `check = "http"` 與 `check_options` | URL 被拆開：位址是 backend 的，路徑是 `path`。Consul 的檢查通常在節點本機跑（`localhost`）；emfor 是從應用程式主機連到 backend 的位址，所以防火牆要放行。 |
| `check { tcp = ... }` | `check = "tcp"` | |
| `check { grpc = ... }` | `emfor-check` 的 `grpc` plugin | 見 [plugin](04-health-checks.md#plugin-與-emfor-check)。 |
| `check { args = [...] }`（script） | `check = "exec"` | |
| `interval` | `interval` | Consul 沒有預設值；emfor 的預設是 2 秒。 |
| `timeout` | `timeout` | Consul 對 HTTP 與 TCP 的預設是 10 秒；emfor 的是 1 秒，而且不能超過 `interval`。 |
| `success_before_passing` | `rise` | Consul 的預設是 0；emfor 的是 2。請明確設定。 |
| `failures_before_critical` | `fall` | Consul 的預設是 0；emfor 的是 3。請明確設定。 |
| `method`、`header` | 無 | emfor 送 `GET`；header 只能設 `Host`（`host_header`）。 |
| `tls_skip_verify = true` | `tls_verify = false` | 搭配 `scheme = "https"`。 |
| 狀態 `passing`、`warning`、`critical` | `healthy`、`unhealthy`、`unknown` | Consul 的 HTTP 檢查在 200 到 299 為 `passing`、429 為 `warning`、其他為 `critical`；emfor 沒有 `warning`：用 `expect_status` 設定你接受的狀態碼。 |
| `status`（初始為 `critical`） | 從 `unknown` 開始 | backend 通過 `rise` 次檢查後才變成 `healthy`；在那之前，`open` 的服務仍會回傳一個位址（[第 5 章](05-backend-selection.md#沒有任何-backend-健康時)）。 |
| `deregister_critical_service_after` | 無 | 清單是靜態的，沒有東西會把 backend 反註冊。 |
| TTL 與 alias 檢查 | 無 | emfor 只執行它自己的檢查。 |
| `api.service.consul` | 服務名稱，搭配 `[dns] zone` | 見上面。 |
| `_api._tcp.service.consul`（SRV） | `_<任意>._tcp.<服務名稱>` | emfor 的 SRV 名稱包含完整的服務名稱，例如 `_http._tcp.api.service.consul`（[第 7 章](07-dns.md)）。 |
| tag、datacenter | `site` 與 `balance = "locality"` | backend 可以有 `site`，主機會優先選自己 site 的 backend（[第 5 章](05-backend-selection.md)）。 |

### 與 Consul 並排試用

步驟和 HAProxy 那節一樣：寫好設定、跑 `emfor check-config`、在 Consul agent 旁邊啟動 daemon、把 `emfor status` 顯示的健康狀態與 `consul catalog` 及 Consul UI 比對，然後一次搬一個應用程式。兩邊不一致的地方正是有用的資訊：它表示這個 backend 從應用程式看過去，與從 Consul agent 看過去是不一樣的。

## 沒有對應的東西

| 你用的功能 | 在 emfor 裡 |
| --- | --- |
| 最少連線、連線數限制、排隊 | 沒有。emfor 不在資料路徑上，看不到連線。這部分請保留 HAProxy。 |
| proxy 的重試與逾時 | 由應用程式重試。client 每次查詢回傳一個位址，所以在下一次檢查執行之前，重試可能拿到同一個 backend。 |
| 被動健康檢查（`observe layer7`） | 沒有：只有主動檢查。失敗的 backend 由下一次檢查發現；在[聯防群組](03-configuration.md#peers)裡，其中一台看到失敗後，各主機每 200ms 會重新檢查。 |
| 為了維護把 backend 拿下來（`disabled`、weight 0、`maint`） | 目前沒有：從清單移除再 reload。重新加回的 backend 會從 `unknown` 開始。 |
| 依 URL、header 或 cookie 路由，TLS 終結 | 沒有。 |
| Consul 的註冊、tag 與 key-value store | 沒有。backend 清單就是設定。 |
| 多個設定檔 | 目前只有一個檔案。 |
| 集中檢視所有檢查 | 每台主機各有自己的。用 [metrics](08-operations.md#metrics) 搭配 Prometheus。 |

表裡寫「請保留 HAProxy」的地方，也可以兩者並用：emfor 在幾個 HAProxy 實例之間選擇，每個 HAProxy 再在它的 server 之間選擇。
