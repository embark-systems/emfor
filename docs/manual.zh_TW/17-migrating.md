# 17. 從 HAProxy 或 Consul 搬過來

[手冊](README.md) · 上一章：[16. 疑難排解](16-troubleshooting.md) · 下一章：[18. 設定參考](18-config-reference.md)

HAProxy 和 Consul 做的事，emfor 只做其中一部分：在每台應用程式主機上檢查一份固定的 backend 清單，再告訴應用程式該用哪一台。下面對 HAProxy 和 Consul 的說明，是照 repository 文件裡的描述寫的；寫這份手冊時沒有拿實際產品核對過。

| | HAProxy | Consul | emfor |
| --- | --- | --- | --- |
| 流量走哪裡 | 經過 HAProxy | 直接到 backend | 直接到 backend |
| 誰決定用哪個 backend | HAProxy，每條連線都由它決定 | 應用程式，依 DNS | 應用程式，依本機 store 或 DNS |
| 檢查在哪裡跑 | 在 HAProxy 裡 | 在服務旁邊的 agent 裡 | 在每台應用程式主機上 |
| backend 清單 | 設定檔 | 服務自己註冊 | 每台主機的設定檔 |
| 中央伺服器 | 沒有 | 有 | 沒有 |

適合用 emfor 的情況是：backend 是一份已知的位址清單（虛擬機、實體機、資料庫、AAA server），而且應用程式可以自己問該連到哪裡。proxy 對流量做的事，emfor 取代不了：最少連線、重試、排隊、TLS 終結、依 URL 或 header 路由、連線數限制、被動檢查。需要這些功能就繼續用 HAProxy，也可以讓 emfor 把資料餵給它（[第 14 章](14-integrations.md#haproxy)）。

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
| `backend api` | `[services."api.svc.emfor"]` | 這也是 DNS 名稱。 |
| `server ... 10.0.0.11:8080` | `backends` 裡的一個項目 | 只能寫 IP 位址。 |
| `check` | `check = "http"` 或 `"tcp"` | 每個服務只有一種檢查。 |
| `inter`、`fall`、`rise` | `interval`、`fall`、`rise` | 預設是 2 s、3、2。 |
| `option httpchk GET /healthz` | `path = "/healthz"` | emfor 一律送 `GET`。 |
| `http-check expect status 200` | `expect_status = ["200"]` | 預設是 `200-399`。 |
| `hdr Host x` | `host_header = "x"` | 只能設 `Host`。 |
| `weight 3` | `weight = 3` | 1–100；`weight = 0` 會被拒絕，請改用 `drain = true`。`weighted` 和有 key 的 `sticky` 會用到權重。 |
| `backup` | `backup = true` | |
| `slowstart 30s` | `slow_start = "30s"` | 設在服務上，分十個階段。 |
| 有權重的 `balance roundrobin` | `balance = "weighted"` | emfor 的 `round-robin` 不看權重。 |
| `balance first` | `balance = "failover"` | |
| `balance source`、`uri`、`hdr` | `balance = "sticky"` | key 由應用程式傳入。 |
| `balance leastconn`、`maxconn`、佇列、`observe layer7` | 無 | emfor 看不到連線。 |
| `check port 8081` | 無 | emfor 檢查的是 backend 本身的位址和 port（不然就用 `exec` 或 plugin）。 |
| `agent-check` | `exec` 或 plugin | |
| `disabled`、`set server ... state drain` | `drain = true`，再 reload | 沒有執行期的指令可用。 |

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

在 emfor 裡可以寫成一樣的設定，並沿用 Consul 的 DNS 名稱。這樣只要主機的 resolver 把 `service.consul` 轉送給 emfor，程式就不用改：

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
| 註冊（`service { name, port }`） | 設定檔裡的 `backends` | 不會有任何東西來註冊；清單由你的組態管理工具寫出來。要加節點，就改檔案再 reload。 |
| `check { http = ... }` | `check = "http"` | emfor 是從應用程式主機連到 backend 的位址，不是連節點上的 `localhost`：記得開防火牆。 |
| `check { tcp = ... }` | `check = "tcp"` | |
| `check { grpc = ... }` | `emfor-check` 的 `grpc` 檢查 | |
| `check { args = [...] }` | `check = "exec"` | |
| `interval`、`timeout` | `interval`、`timeout` | emfor 的 `timeout` 預設 1 s，而且不能超過 `interval`。 |
| `success_before_passing`、`failures_before_critical` | `rise`、`fall` | 在 emfor 裡最小是 1（`rise = 1` 就是「第一個結果說了算」）。 |
| `tls_skip_verify` | `tls_verify = false` | 搭配 `scheme = "https"`。 |
| `passing`、`warning`、`critical` | `healthy`、`unhealthy`、`unknown` | 沒有 `warning`：你能接受哪些狀態碼，就列在 `expect_status` 裡。 |
| 維護模式 | `drain = true` | |
| tag、datacenter | `site`、`balance = "locality"` | |
| `_api._tcp.service.consul`（SRV） | `_<anything>._tcp.api.service.consul` | |
| TTL 與 alias 檢查、`deregister_critical_service_after`、KV | 無 | |

搭配 `include_dir`，就可以一個服務放一個檔案（[第 5 章](05-configuring.md#把設定拆到目錄)）。要注意，`backends` 清單會被最後一個設定它的檔案整份蓋掉。

## 讓 emfor 與舊系統並行試用

emfor 不碰流量，所以可以讓新舊系統同時跑一陣子：

1. 寫好服務設定，跑 `emfor check-config`，再啟動 daemon。
2. 拿 `emfor status` 和 HAProxy 的統計頁或 Consul 的 UI 對照。有差異反而有用：代表從應用程式主機看到的 backend，和從另一套系統做檢查的位置看到的不一樣。
3. 用 `emfor lookup <service> --explain` 看應用程式會拿到什麼。
4. 一次只把一個應用程式改成用 client 函式庫、`emfor lookup` 或 DNS，同時盯著 metrics。
5. 確定已經沒有任何東西在用某個 backend，才把它從舊系統拿掉。

## 應用程式必須自己做的事

- **重試。** 一次查詢只回一個位址。backend 只要有一次檢查失敗就會被排除（前提是還有別台通過），所以稍等一下再重試，通常就會拿到另一台；如果用 `round-robin`，同一個行程裡的重試本來就會拿到下一個候選。
- **自己記住 port**：如果用的是 A/AAAA DNS 回答，回答裡不會帶 port（也可以改用 SRV 或 client 函式庫）。
- **每條連線都查一次**，不要只在啟動時查一次。
