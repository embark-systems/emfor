# 8. 如何選出 backend

[手冊](README.md) · 上一章：[7. Plugin](07-plugins.md) · 下一章：[9. Client 函式庫](09-clients.md)

一次查詢，不論是透過 client 函式庫、`emfor lookup` 還是 DNS，都分三步：

1. 從 store 讀取服務記錄與 heartbeat（DNS 使用 daemon 的記憶體）。
2. 找出**候選**：第一個不是空的[層級](#層級)。
3. 由服務的 **balance** 方式從候選中選出一個。

`emfor lookup <service> --explain` 會印出每一步。本章逐一說明。

## 層級

| # | 層級（`--explain` 印出的樣子） | 何時使用 |
| --- | --- | --- |
| 1 | `healthy primary backends whose last check passed` | 永遠最先 |
| 2 | `healthy backup backends whose last check passed` | 層級 1 是空的 |
| 3 | `healthy primary backends including those whose last check failed` | 層級 1–2 是空的 |
| 4 | `healthy backup backends including those whose last check failed` | 層級 1–3 是空的 |
| 5 | `unknown backends whose last check passed` | 層級 1–4 是空的，且 `on_all_down = "open"` |
| 6 | `every unknown backend` | 同 5，且層級 5 是空的 |
| 7 | `every backend` | 同 5，且層級 6 是空的 |
| – | `none` | 層級 1–4 是空的，且 `on_all_down = "closed"`：查詢以 `ALL_DOWN` 失敗 |

**drain** 的 backend 不在任何層級中，連層級 7 也沒有。所以 backend 全部被 drain 的服務，即使 `on_all_down = "open"`，也一定得到 `ALL_DOWN`。

store **過期**（stale）時（heartbeat 比 `EMFOR_MAX_STALENESS` 舊，預設 3 秒，或因為 daemon 已停止而為 0），client 把每個 backend 都當作不健康，所以層級 1–4 是空的：`open` 的服務從層級 7（每個沒有被 drain 的 backend）回答，`closed` 的服務以
`ALL_DOWN: service db.svc.emfor: the store is stale (emfor daemon not updating it), and on_all_down is closed` 失敗。

`--explain` 會為每個不是候選的 backend 說明原因：

| 顯示為 | 意義 |
| --- | --- |
| `unhealthy` | 它的狀態是 `unhealthy`。 |
| `unknown` | 它的狀態是 `unknown`。 |
| `last check failed` | `healthy`（在層級 5 中則是 `unknown`），但最近一次檢查沒有通過，而其他 backend 通過了。 |
| `backup held back` | 是備援，而有主要 backend 是候選。 |
| `drained` | `drain = true`。 |
| `stale` | store 已過期。 |
| `not in the pool` | `fastest`：不在最快的 backend 之中。 |
| `not at this site` | `locality`：在別的 site，而這台主機所在 site 有 backend 是候選。 |

## on_all_down

| 值 | 層級 1–4 是空的時 |
| --- | --- |
| `open`（預設） | 從層級 5–7 回答。DNS 也會回答。 |
| `closed` | 查詢以 `ALL_DOWN` 失敗；DNS 回答 `SERVFAIL`。 |

試一個 backend 比回錯誤好（而且你的檢查可能出錯）的無狀態服務，選 `open`；錯的 backend 會造成傷害的地方，例如可能已被降級的資料庫 primary，選 `closed`。

## balance 方式

balance 方式依服務選擇（或在 `[defaults]` 中設定）：

```toml
[services."api.svc.emfor"]
check = "http"
balance = "weighted"
backends = [
  { addr = "10.0.0.11:8080", weight = 3 },
  { addr = "10.0.0.12:8080", weight = 1 },
]
```

### round-robin（預設）

每個行程為每個服務保留一個計數器，起始值隨機，輪流取用候選。不同的行程之間不協調，所以只有在很多行程或很多次查詢之下，分配才會平均。

### sticky

`sticky` 對同一個 **affinity key** 給出同一個 backend；affinity key 是你的應用程式傳入的字串（使用者 ID、session ID、租戶）：

```python
addr = emfor.lookup("cache.svc.emfor", affinity_key="user:42")
```

選擇方式是對候選做加權的 **rendezvous hash**：每個候選得到分數 `MD5(key + id)`（前 4 個位元組，big-endian；`id` 是 backend 的 `ip:port`），每個 backend 有 `weight` 個分數，分數最高的勝出。因此：

- 每台主機上的每個行程對同一個 key 都選到同一個 backend，不需要共用狀態；
- backend 離開候選時，只有原本在它上面的 key 會移走；它回來時，這些 key 也移回來；
- `weight = 2` 的 backend 分到大約兩倍的 key。

沒有 key（或 key 為空）時，以及在 DNS 中，`sticky` 的行為同 `round-robin`。慢啟動不改變一個 key 的選擇。

### failover

**設定檔順序中的第一個候選**。把偏好的 backend 列在最前面：

```toml
[services."db.svc.emfor"]
check = "tcp"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
```

第一個 backend 檢查失敗時，它立刻離開層級 1，改選第二個。第一個再次通過且為 `healthy` 時，它又會被選到，所以流量會自動**切回**（fail back）。如果你不要這樣，請用只在目前的 primary 上才會通過的檢查（`pgsql`、`mysql`、`redis` 或 `mongodb` 檢查的 `role = "primary"`，[第 7 章](07-plugins.md#角色與複寫延遲)）。

### weighted

每個候選被選中的比例與它的 `weight`（1–100，預設 1）成正比。

- client 函式庫取 `counter mod (sum of weights)`，並依設定檔順序走過候選：權重為 3 與 1 時，一個行程給出 `a a a b a a a b ...`。
- DNS 使用 smooth weighted round-robin，把它們分散開來：`a a b a a a b a ...`。

只計算候選：權重大的 backend 不健康時，其他 backend 依它們的權重分擔流量。

### locality

`site` 等於主機的 `site`（設定在設定檔的最上層）的候選輪流被選中；沒有這樣的候選時，所有候選輪流被選中。

```toml
site = "dc1"

[services."api.svc.emfor"]
check = "http"
balance = "locality"
backends = [
  { addr = "10.1.0.11:8080", site = "dc1" },
  { addr = "10.1.0.12:8080", site = "dc1" },
  { addr = "10.2.0.11:8080", site = "dc2" },
]
```

這裡在 `dc1` 主機上的查詢，只要兩台 `dc1` backend 中有任何一台是候選，就給出這兩台；只有兩台都不在時才給 `dc2` 那台。層級優先：最近一次檢查失敗的 `dc1` backend，不會優先於通過檢查的 `dc2` backend。沒有 `site` 的主機，或沒有 `site` 的 backend，得到的是一般的 round-robin。

### fastest

daemon 為每個服務維護一個 **pool**，裡面是回答檢查最快的 backend，client 輪流取用 pool 中的候選。

- 每個 backend 的延遲是它檢查時間的移動平均（每個新樣本佔 5%），只計算 `healthy` 的 backend。
- backend 的延遲最多是最低值的 1.5 倍時（至少是最低值 + 1 ms）加入 pool，超過最低值的 2 倍時（至少是最低值 + 2 ms）離開。這個差距讓 backend 不會反覆進出。
- 主要 backend 與備援各有自己的 pool。
- pool 中沒有候選，或 store 已過期時，`fastest` 的行為同 `round-robin`。

`emfor status` 會標出在 pool 中的 backend。延遲是**檢查**的延遲，在這台主機上量測：`tcp` 連線很快，並不代表應用程式回答得多快。

## 備援 backend

```toml
[services."api.svc.emfor"]
check = "http"
backends = [
  "10.0.0.11:8080",
  "10.0.0.12:8080",
  { addr = "10.9.0.11:8080", backup = true },
]
```

備援只在層級 2 與 4 中是候選：也就是沒有主要 backend 通過檢查的時候。至少要有一個 backend 不是備援（`at least one backend must not be a
backup`）。備援和其他 backend 一樣會被檢查。

## 慢啟動

```toml
[services."api.svc.emfor"]
check = "http"
slow_start = "30s"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
```

backend 從 `unhealthy` 轉為 `healthy` 時，一開始只拿到它份額的 1/10，之後每 `slow_start / 10`（這裡是每 3 秒）多十分之一，直到 30 秒後拿到全部份額。從 `unknown` 轉為健康的 backend（啟動或 reload 之後）立刻拿到全部份額。

- `slow_start` 是 `0s`（關閉，預設），或 `1s` 到 `1h`。
- 有候選正在逐步增加時，除了 `failover`（以及有 key 的 `sticky`）之外，每種方式都依有效權重選擇：`weighted` 是 `weight × step`，其他方式是 `step`。
- `--explain` 列出正在慢啟動的候選，以及它們的步數與有效權重。
- store 過期時，該次查詢不做慢啟動。

## drain

```toml
[services."api.svc.emfor"]
check = "http"
backends = [
  "10.0.0.11:8080",
  { addr = "10.0.0.12:8080", drain = true },
]
```

drain 的 backend 絕不會被選到，不論是 client 還是 DNS，但它的檢查照常進行，所以 `emfor status` 仍然顯示它的狀況：

```text
  127.0.0.2:8080  drained    2.2s ago  2.4ms  (checks: healthy)
```

改變 `drain` 只需要 reload，而且不會重設 backend 的狀態。在 store 中，drain 的 backend 寫成 `"state": "unhealthy"`，reason 為 `drained`，所以較舊發行版的 client 也會略過它。已經與它建立連線的應用程式會保留那條連線：emfor 不在資料路徑上。[第 12 章](12-operating.md#用-drain-做滾動部署)有完整的滾動部署做法。

## DNS 與 client 函式庫的比較

| | client 函式庫、`emfor lookup` | DNS |
| --- | --- | --- |
| 讀取 | store | daemon 的記憶體 |
| 回答 | 一個 `ip:port` | A/AAAA：一個位址，不帶 port。SRV：每個候選。 |
| `sticky` | 依 affinity key | 同 round-robin |
| `weighted` | 以計數器對累計權重取值 | smooth weighted round-robin；SRV 帶有權重 |
| `failover` | 第一個候選 | 第一個；SRV 中依設定檔順序為 priority 0、1、2 |
| `locality`、`fastest` | 輪流使用偏好的候選 | 相同；SRV 中偏好的候選 priority 為 0，其他為 1 |
| daemon 停止 | store 過期：由 `on_all_down` 決定 | 完全沒有回答 |
| 快取 | 無：每次查詢都讀 store | 由 resolver 快取 `ttl` 的時間（預設 1 秒） |

DNS 回答的細節見[第 10 章](10-dns.md)。
