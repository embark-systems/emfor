# 8. 如何選出 backend

[手冊](README.md) · 上一章：[7. Plugin](07-plugins.md) · 下一章：[9. Client 函式庫](09-clients.md)

不管是用 client 函式庫、`emfor lookup` 還是 DNS 查詢，都分三步：

1. 從 store 讀出服務記錄和 heartbeat（DNS 則是用 daemon 記憶體裡的資料）。
2. 找出**候選**：也就是第一個不是空的[層級](#層級)。
3. 用服務設定的 **balance** 方式，從候選裡挑一個。

`emfor lookup <service> --explain` 會把每一步都印出來。這一章逐步說明。

## 層級

| # | 層級（`--explain` 印出的樣子） | 何時使用 |
| --- | --- | --- |
| 1 | `healthy primary backends whose last check passed` | 一定最先用 |
| 2 | `healthy backup backends whose last check passed` | 層級 1 是空的 |
| 3 | `healthy primary backends including those whose last check failed` | 層級 1–2 是空的 |
| 4 | `healthy backup backends including those whose last check failed` | 層級 1–3 是空的 |
| 5 | `unknown backends whose last check passed` | 層級 1–4 是空的，且 `on_all_down = "open"` |
| 6 | `every unknown backend` | 同 5，且層級 5 是空的 |
| 7 | `every backend` | 同 5，且層級 6 是空的 |
| – | `none` | 層級 1–4 是空的，且 `on_all_down = "closed"`：查詢失敗，回傳 `ALL_DOWN` |

被 **drain** 的 backend 不屬於任何層級，連層級 7 也不算。所以一個服務的 backend 如果全部被 drain，就算 `on_all_down = "open"`，也一定拿到 `ALL_DOWN`。

store **過期**（stale）是指 heartbeat 比 `EMFOR_MAX_STALENESS` 還舊（預設 3 秒），或是 daemon 已經停止、heartbeat 變成 0。這時 client 會把所有 backend 都當成不健康，層級 1–4 因此都是空的：`open` 的服務從層級 7（所有沒被 drain 的 backend）回答；`closed` 的服務則查詢失敗，回傳
`ALL_DOWN: service db.svc.emfor: the store is stale (emfor daemon not updating it), and on_all_down is closed`。

對每個不是候選的 backend，`--explain` 都會寫出原因：

| 顯示為 | 意義 |
| --- | --- |
| `unhealthy` | 狀態是 `unhealthy`。 |
| `unknown` | 狀態是 `unknown`。 |
| `last check failed` | 狀態是 `healthy`（在層級 5 則是 `unknown`），但最近一次檢查沒過，而其他 backend 過了。 |
| `backup held back` | 這是備援，而目前有主要 backend 是候選。 |
| `drained` | `drain = true`。 |
| `stale` | store 已過期。 |
| `not in the pool` | `fastest`：不在最快的那幾個 backend 裡。 |
| `not at this site` | `locality`：在別的 site，而這台主機所在的 site 有 backend 是候選。 |

## on_all_down

| 值 | 層級 1–4 是空的時 |
| --- | --- |
| `open`（預設） | 從層級 5–7 回答。DNS 也會回答。 |
| `closed` | 查詢失敗，回傳 `ALL_DOWN`；DNS 回答 `SERVFAIL`。 |

無狀態的服務，與其回錯誤不如找個 backend 試試看（何況你的檢查也可能判斷錯），這種就選 `open`。選錯 backend 會出事的地方，例如可能已經被降級的資料庫 primary，就選 `closed`。

## balance 方式

balance 方式是每個服務各自設定（也可以寫在 `[defaults]`）：

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

每個行程替每個服務各留一個計數器，起始值是隨機的，然後輪流選候選。不同行程之間不會互相協調，所以要有很多行程或很多次查詢，分配才會平均。

### sticky

同一個 **affinity key**，`sticky` 會給同一個 backend。affinity key 是你的應用程式傳進來的字串（使用者 ID、session ID、租戶）：

```python
addr = emfor.lookup("cache.svc.emfor", affinity_key="user:42")
```

挑選的方法是對候選做加權的 **rendezvous hash**：每個候選的分數是 `MD5(key + id)`（取前 4 個位元組，big-endian；`id` 是 backend 的 `ip:port`），每個 backend 有 `weight` 個分數，分數最高的就選中。所以：

- 不管在哪台主機、哪個行程，同一個 key 都會選到同一個 backend，不需要共用狀態；
- backend 離開候選時，只有原本落在它身上的 key 會移走；它回來時，這些 key 也會移回來；
- `weight = 2` 的 backend 大約分到兩倍的 key。

沒有 key（或 key 是空的）時，還有在 DNS 裡，`sticky` 的行為跟 `round-robin` 一樣。慢啟動不會改變某個 key 選到的 backend。

### failover

選**設定檔順序裡的第一個候選**。把你偏好的 backend 列在最前面：

```toml
[services."db.svc.emfor"]
check = "tcp"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
```

第一個 backend 檢查失敗時，會馬上離開層級 1，改成選第二個。等第一個再次通過、狀態是 `healthy`，又會選回它，所以流量會自動**切回**（fail back）。如果你不想要這樣，就改用只有目前的 primary 才會通過的檢查（`pgsql`、`mysql`、`redis` 或 `mongodb` 檢查的 `role = "primary"`，[第 7 章](07-plugins.md#角色與複寫延遲)）。

### weighted

每個候選被選中的比例，跟它的 `weight`（1–100，預設 1）成正比。

- client 函式庫算 `counter mod (sum of weights)`，再照設定檔順序走過候選：權重是 3 和 1 時，一個行程選出來的順序是 `a a a b a a a b ...`。
- DNS 用 smooth weighted round-robin，會把它們打散：`a a b a a a b a ...`。

只有候選會列入計算：權重大的 backend 不健康時，其他 backend 照各自的權重分擔流量。

### locality

候選的 `site` 和主機的 `site`（寫在設定檔最上層）相同的，會輪流被選；沒有這種候選時，就在所有候選之間輪流。

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

在這個例子裡，`dc1` 主機上的查詢，只要兩台 `dc1` backend 有任何一台是候選，就會在這兩台之間選；兩台都不在了，才會選 `dc2` 那台。層級優先：最近一次檢查失敗的 `dc1` backend，不會比檢查通過的 `dc2` backend 優先。主機沒設 `site`，或 backend 沒設 `site`，就是一般的 round-robin。

### fastest

daemon 替每個服務維護一個 **pool**，裡面放回答檢查最快的 backend；client 在 pool 裡的候選之間輪流選。

- 每個 backend 的延遲，是它檢查時間的移動平均（每個新樣本佔 5%），只算 `healthy` 的 backend。
- backend 的延遲不超過最低值的 1.5 倍（至少是最低值 + 1 ms）時加入 pool，超過最低值的 2 倍（至少是最低值 + 2 ms）時離開。中間留這段差距，backend 才不會反覆進出。
- 主要 backend 和備援各有各的 pool。
- pool 裡沒有候選，或 store 已過期時，`fastest` 的行為跟 `round-robin` 一樣。

`emfor status` 會標出哪些 backend 在 pool 裡。這裡的延遲是**檢查**的延遲，而且是在這台主機上量的：`tcp` 連線很快，不代表應用程式回答也快。

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

備援只在層級 2 和 4 才會是候選，也就是沒有任何主要 backend 通過檢查的時候。至少要有一個 backend 不是備援（`at least one backend must not be a
backup`）。備援也會跟其他 backend 一樣接受檢查。

## 慢啟動

```toml
[services."api.svc.emfor"]
check = "http"
slow_start = "30s"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
```

backend 從 `unhealthy` 變成 `healthy` 時，一開始只分到原本份額的 1/10，之後每過 `slow_start / 10`（這裡是每 3 秒）就多十分之一，30 秒後拿回全部份額。如果是從 `unknown` 變成健康（啟動或 reload 之後），會馬上拿到全部份額。

- `slow_start` 可以是 `0s`（關閉，預設值），或 `1s` 到 `1h`。
- 有候選正在慢慢加量時，除了 `failover`（以及有 key 的 `sticky`），其他方式都照有效權重來選：`weighted` 是 `weight × step`，其他方式是 `step`。
- `--explain` 會列出正在慢啟動的候選，以及各自的步數和有效權重。
- store 過期時，那次查詢不套用慢啟動。

## drain

```toml
[services."api.svc.emfor"]
check = "http"
backends = [
  "10.0.0.11:8080",
  { addr = "10.0.0.12:8080", drain = true },
]
```

被 drain 的 backend，不管是 client 還是 DNS 都不會選到它，但檢查照常在跑，所以 `emfor status` 還是看得到它的狀況：

```text
  127.0.0.2:8080  drained    2.2s ago  2.4ms  (checks: healthy)
```

改 `drain` 只要 reload，也不會重設 backend 的狀態。被 drain 的 backend 在 store 裡寫成 `"state": "unhealthy"`、reason 是 `drained`，這樣舊版的 client 也會跳過它。已經連上它的應用程式會繼續用那條連線，因為 emfor 不在資料路徑上。完整的滾動部署做法請看[第 12 章](12-operating.md#用-drain-做滾動部署)。

## DNS 與 client 函式庫的比較

| | client 函式庫、`emfor lookup` | DNS |
| --- | --- | --- |
| 讀取 | store | daemon 的記憶體 |
| 回答 | 一個 `ip:port` | A/AAAA：一個位址，不帶 port。SRV：每個候選。 |
| `sticky` | 依 affinity key | 同 round-robin |
| `weighted` | 用計數器在累計權重上取值 | smooth weighted round-robin；SRV 會帶權重 |
| `failover` | 第一個候選 | 第一個；SRV 裡照設定檔順序給 priority 0、1、2 |
| `locality`、`fastest` | 在偏好的候選之間輪流 | 一樣；SRV 裡偏好的候選 priority 是 0，其他是 1 |
| daemon 停止 | store 過期，由 `on_all_down` 決定 | 完全不回答 |
| 快取 | 沒有：每次查詢都讀 store | 由 resolver 快取 `ttl` 這麼久（預設 1 秒） |

DNS 回答的細節請看[第 10 章](10-dns.md)。
