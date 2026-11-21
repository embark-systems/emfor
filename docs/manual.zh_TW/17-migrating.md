# 17. 從 HAProxy 或 Consul 搬過來

[手冊](README.md) · 上一章：[16. 疑難排解](16-troubleshooting.md) · 下一章：[18. 設定參考](18-config-reference.md)

HAProxy 與 Consul 做的事裡，emfor 只做一部分：從每台應用程式主機檢查一份固定的 backend 清單，並告訴應用程式該用哪一台。下面關於 HAProxy 與 Consul 的說明，是依照 repository 的文件描述這些產品；這份手冊沒有拿它們實際核對過。

| | HAProxy | Consul | emfor |
| --- | --- | --- | --- |
| 流量走哪裡 | 經過 HAProxy | 直接到 backend | 直接到 backend |
| 誰決定用哪個 backend | HAProxy，每條連線都由它決定 | 應用程式，依 DNS | 應用程式，依本機 store 或 DNS |
| 檢查在哪裡跑 | 在 HAProxy 裡 | 在服務旁邊的 agent 裡 | 在每台應用程式主機上 |
| backend 清單 | 設定檔 | 服務自己註冊 | 每台主機的設定檔 |
| 中央伺服器 | 沒有 | 有 | 沒有 |

emfor 適合的情況是：backend 是已知的一份位址清單（虛擬機、實體機、資料庫、AAA server），而且應用程式能問該連到哪裡。它取代不了 proxy 對流量做的事：最少連線、重試、排隊、TLS 終結、依 URL 或 header 路由、連線數限制、被動檢查。這些請保留 HAProxy，也可以讓 emfor 餵給它（[第 14 章](14-integrations.md#haproxy)）。

## 從 HAProxy 搬過來

這個 backend：

```text
backend api
    balance roundrobin
    option httpchk GET /healthz
    http-check expect status 200
    default-server inter 2s fall 3 rise 2
    server api1 10.0.0.11:8080 check
    server api2 10.0.0.12:8080 check weight 3
    server api3 10.0.1.11:8080 check backup slowstart 30s
```

會變成：

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
expect_status = ["200"]
```

| HAProxy | emfor | 說明 |
| --- | --- | --- |
| `backend api` | `[services."api.svc.emfor"]` | 同時也是 DNS 名稱。 |
| `server ... 10.0.0.11:8080` | `backends` 的一個項目 | 只能寫 IP 位址。 |
| `check` | `check = "http"` 或 `"tcp"` | 每個服務一個檢查。 |
| `inter`、`fall`、`rise` | `interval`、`fall`、`rise` | 預設為 2 s、3、2。 |
| `option httpchk GET /healthz` | `path = "/healthz"` | emfor 一律送 `GET`。 |
| `http-check expect status 200` | `expect_status = ["200"]` | 預設 `200-399`。 |
| `hdr Host x` | `host_header = "x"` | 只能設定 `Host`。 |
| `weight 3` | `weight = 3` | 1–100；`weight = 0` 會被拒絕：請用 `drain = true`。`weighted` 與有 key 的 `sticky` 會用到。 |
| `backup` | `backup = true` | |
| `slowstart 30s` | `slow_start = "30s"` | 設在服務上，分十個階段。 |
| 有權重的 `balance roundrobin` | `balance = "weighted"` | emfor 的 `round-robin` 會忽略權重。 |
| `balance first` | `balance = "failover"` | |
| `balance source`、`uri`、`hdr` | `balance = "sticky"` | key 由應用程式提供。 |
| `balance leastconn`、`maxconn`、佇列、`observe layer7` | 無 | emfor 看不到連線。 |
| `check port 8081` | 無 | emfor 檢查的是 backend 自己的位址與 port（或用 `exec` 或 plugin）。 |
| `agent-check` | `exec` 或 plugin | |
| `disabled`、`set server ... state drain` | `drain = true`，然後 reload | 沒有執行期的指令。 |

## 從 Consul 搬過來

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

在 emfor 裡寫成同樣的東西，並保留 Consul 的 DNS 名稱，這樣主機的 resolver 把 `service.consul` 轉送給 emfor 之後，程式就不必修改：

```toml
[dns]
zone = "service.consul"

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
```

| Consul | emfor | 說明 |
| --- | --- | --- |
| 註冊（`service { name, port }`） | 設定檔中的 `backends` | 沒有東西會註冊；由你的組態管理工具寫出清單。要加入節點，就修改檔案並 reload。 |
| `check { http = ... }` | `check = "http"` | emfor 從應用程式主機連到 backend 的位址，而不是節點上的 `localhost`：請開放防火牆。 |
| `check { tcp = ... }` | `check = "tcp"` | |
| `check { grpc = ... }` | `emfor-check` 的 `grpc` 檢查 | |
| `check { args = [...] }` | `check = "exec"` | |
| `interval`、`timeout` | `interval`、`timeout` | emfor 的 `timeout` 預設 1 s，且不能超過 `interval`。 |
| `success_before_passing`、`failures_before_critical` | `rise`、`fall` | 在 emfor 中至少為 1（`rise = 1` 是「第一個結果就決定」）。 |
| `tls_skip_verify` | `tls_verify = false` | 搭配 `scheme = "https"`。 |
| `passing`、`warning`、`critical` | `healthy`、`unhealthy`、`unknown` | 沒有 `warning`：把你接受的狀態碼列在 `expect_status`。 |
| 維護模式 | `drain = true` | |
| tag、datacenter | `site`、`balance = "locality"` | |
| `_api._tcp.service.consul`（SRV） | `_<anything>._tcp.api.service.consul` | |
| TTL 與 alias 檢查、`deregister_critical_service_after`、KV | 無 | |

用 `include_dir` 可以一個服務一個檔案（[第 5 章](05-configuring.md#把設定拆到目錄)）；注意 `backends` 清單會被最後一個設定它的檔案整個取代。

## 讓 emfor 與舊系統並行試用

emfor 不碰流量，所以可以讓兩者同時跑一段時間：

1. 寫好服務，執行 `emfor check-config`，啟動 daemon。
2. 把 `emfor status` 和 HAProxy 的統計頁或 Consul 的 UI 比較。有差異是有用的：從應用程式主機看到的 backend，和從另一個系統檢查它的地方看到的不一樣。
3. 用 `emfor lookup <service> --explain` 看應用程式會得到什麼。
4. 一次把一個應用程式改用 client 函式庫、`emfor lookup` 或 DNS，同時觀察 metrics。
5. 只在沒有任何東西使用某個 backend 時，才把它從舊系統移除。

## 應用程式必須自己做的事

- **重試。** 一次查詢回傳一個位址。backend 在它第一次檢查失敗時就退出（在另一個通過時），所以稍後的重試通常會拿到另一個；用 `round-robin` 時，同一個行程裡的重試本來就會拿到下一個候選。
- **自己記住 port**，如果它們用的是 A/AAAA DNS 回答，因為這種回答不帶 port（或改用 SRV，或 client 函式庫）。
- **每條連線查詢一次**，而不是只在啟動時查一次。
