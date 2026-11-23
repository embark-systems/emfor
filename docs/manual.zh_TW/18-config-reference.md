# 18. 設定參考

[手冊](README.md) · 上一章：[17. 搬移](17-migrating.md) · 下一章：[19. 詞彙表](19-glossary.md)

這裡列出 emfor 0.7.0 接受的每一個 key。除此之外的 key 都會報錯（`<path>: unknown key`）。各種檢查的選項請看[第 6 章](06-health-checks.md)（`tcp`、`http`、`exec`）和[第 7 章](07-plugins.md)（`emfor-check`）。

## 值的格式

| 種類 | 格式 | 範例 |
| --- | --- | --- |
| 時間長度 | 由一或多組「數字加單位」組成的字串；單位是 `ms`、`s`、`m`、`h`（也可以寫全名，如 `seconds`、`minutes`……）。可以單獨寫 `"0"`。只寫數字會被拒絕。 | `"500ms"`、`"1.5s"`、`"1h 30m"` |
| 百分比 | 帶 `%` 的字串 | `"10%"` |
| 大小 | 帶 `B`、`KiB`、`MiB` 或 `GiB` 的字串 | `"64MiB"` |
| 檔案權限 | 八進位字串，最大 `0777` | `"0640"` |
| 位址 | `"IPv4:port"` 或 `"[IPv6]:port"`，port 為 1–65535，不能寫主機名稱 | `"10.0.0.11:8080"`、`"[2001:db8::11]:8080"` |
| site | 1–63 個 `a-z`、`0-9`、`-` 字元，開頭和結尾不能是 `-` | `"dc1"` |
| 服務名稱 | 由 `a-z`、`0-9`、`-` 組成的 label（每個 1–63 個字元），用點隔開，總長最多 253 個字元；不分大小寫，結尾的點會被忽略 | `"db.svc.emfor"` |

## 最上層

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `site` | site | 無 | 這台主機所在的資料中心。有 `[peers]` 時一定要設；`locality` 會用到。 |
| `include_dir` | 路徑 | 無 | 一個目錄，裡面的 `*.toml` 檔會依檔名順序合併進來；相對路徑以主設定檔為準。**只能寫在主設定檔。**一定要寫在第一個 table 之前。 |

## [store]

**只能寫在主設定檔。這裡的 key 都不能用 reload 改。**

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `path` | 路徑 | `/var/lib/emfor/state.mdb` | client 讀的是 `EMFOR_STORE`，所以兩邊要設成一樣。目錄必須已經存在。 |
| `map_size` | 大小 | `"64MiB"` | 至少 `1MiB`，會無條件進位到 64 KiB。要設多大見[第 12 章](12-operating.md#估算-store-要多大)。 |
| `file_mode` | 檔案權限 | `"0640"` | 每次啟動時套用到 store 上。 |
| `lock_file_mode` | 檔案權限 | `"0660"` | 每次啟動時套用到 `<path>-lock` 上。設成其他人可寫的權限會有警告。 |
| `sync` | 布林值 | `false` | 設成 `true` 時，每次 commit 都會先寫進磁碟才算完成。 |

## [log]

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `level` | `"trace"`、`"debug"`、`"info"`、`"warn"`、`"error"` | `"info"` | 設了 `RUST_LOG` 會蓋過它。 |
| `format` | `"text"`、`"json"` | `"text"` | `"json"` 是一行一個 JSON 物件（[第 12 章](12-operating.md#json-行)）。要 restart 才能改；reload 改它會被拒絕。 |

## [dns]

有這個 table，DNS server 才會啟動。

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `listen` | 位址陣列 | `["127.0.0.1:5300"]` | 不能是空的，也不能重複。要改得重新啟動。 |
| `zone` | 名稱 | `"svc.emfor"` | 最多 242 個字元。 |
| `ttl` | 時間長度，整數秒 | `"1s"` | 0–3600。 |
| `rate_limit` | 整數 | `100` | 每個 /24 或 /56 每秒最多回幾個 UDP 回答；0–1,000,000；`0` 代表關閉。 |

## [metrics]

有這個 table，metrics server 才會啟動。

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `listen` | 位址陣列 | `["127.0.0.1:5301"]` | 要改得重新啟動。 |

## [peers]

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `listen` | 位址（字串） | 必填 | 不能是 `0.0.0.0` 或 `::`；必須是 `members` 裡的其中一個。要改得重新啟動。 |
| `members` | 位址陣列 | 必填 | 2–64 個，不能重複，全部要是同一種位址家族。 |
| `secret_file` | 絕對路徑 | 必填 | 擁有者是 root 或 daemon 的使用者；其他人不能讀；group 與其他人不能寫；大小 32 bytes 到 64 KiB。 |
| `previous_secret_file` | 絕對路徑 | 無 | 規則同上。只在啟動時讀取。 |
| `encrypt` | 布林值 | `true` | |
| `accept_plaintext` | 布林值 | `encrypt` 開啟時為 `false`，否則為 `true` | 設成 `false` 又搭配 `encrypt = false` 會報錯。 |
| `cipher` | 字串 | `"chacha20-poly1305"` | `"chacha20-poly1305"` 或 `"aes-256-gcm"`；後者要搭配 `encrypt = true`。見[第 11 章](11-peer-groups.md#選用-aes-gcm)。 |

## [defaults]

所有服務共用的設定；每一項都可以在個別服務裡覆寫。

| Key | 種類 | 預設值 | 規則 |
| --- | --- | --- | --- |
| `interval` | 時間長度 | `"2s"` | 100 ms 到 24 h。 |
| `timeout` | 時間長度 | `"1s"` | 至少 10 ms，不能超過 `interval`。 |
| `rise` | 整數 | `2` | 至少 1。 |
| `fall` | 整數 | `3` | 至少 1。 |
| `jitter` | 百分比 | `"10%"` | 0–50%。 |
| `on_all_down` | `"open"`、`"closed"` | `"open"` | |
| `balance` | `"round-robin"`、`"sticky"`、`"failover"`、`"weighted"`、`"locality"`、`"fastest"` | `"round-robin"` | |
| `peers` | 布林值 | `true` | 設成 `false`，服務在這台主機上就不參加聯防群組。 |
| `slow_start` | 時間長度 | `"0s"` | `0s`，或 1 s 到 1 h。 |

## [plugins."NAME"]

`NAME` 是 1–63 個 `a-z`、`0-9`、`_`、`-` 字元，不能用 `tcp`、`http` 或 `exec`。

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `command` | 字串陣列 | 必填 | 第一個元素是可執行檔的絕對路徑。 |
| `env_pass` | 變數名稱陣列 | `[]` | 不能列 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`。改了它，plugin 會重新啟動。 |

## [services."NAME"]

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `check` | `"tcp"`、`"http"`、`"exec"` 或 plugin 名稱 | 必填 | |
| `backends` | 位址或 backend table 的陣列 | 必填 | 至少一個；不能重複；至少要有一個不是備援。 |
| `check_options` | table | `{}` | 內容看 `check` 而定。 |
| `dns_ttl` | 時間長度，整數秒 | `[dns] ttl` | 0–3600。不能寫在 `[defaults]`。 |
| `[defaults]` 的每個 key | | 沿用 `[defaults]` | |

backend table：

| Key | 種類 | 預設值 | 說明 |
| --- | --- | --- | --- |
| `addr` | 位址 | 必填 | |
| `weight` | 整數 | `1` | 1–100。`weighted`（包括它的慢啟動）和有 key 的 `sticky` 會用到；其他方式不看它。 |
| `site` | site | 無 | `locality` 會用到。 |
| `backup` | 布林值 | `false` | 只有在沒有任何主要 backend 通過檢查時才會用。 |
| `drain` | 布林值 | `false` | 永遠不會被選到，但檢查照常進行。用 reload 就能改，狀態不會重設。 |

## reload

改 `[store]`、改 `[dns]`、`[metrics]` 和 `[peers]` 的 `listen` key，或新增、移除這幾個 table，都要重新啟動；其他所有變更都會在 reload 時套用。每種變更對 backend 狀態有什麼影響，請看[第 5 章](05-configuring.md#套用變更reload)。

## 完整範例

```toml
site = "dc1"

[store]
path = "/var/lib/emfor/state.mdb"
map_size = "64MiB"

[log]
level = "info"

[dns]
listen = ["127.0.0.1:5300"]
zone = "svc.emfor"
ttl = "1s"

[metrics]
listen = ["127.0.0.1:5301"]

[defaults]
interval = "2s"
timeout = "1s"
rise = 2
fall = 3
jitter = "10%"

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."api.svc.emfor"]
check = "http"
balance = "locality"
slow_start = "30s"
backends = [
  { addr = "10.1.0.11:8080", site = "dc1" },
  { addr = "10.1.0.12:8080", site = "dc1", weight = 2 },
  { addr = "10.2.0.11:8080", site = "dc2", backup = true },
]
check_options = { path = "/healthz", expect_status = ["200"] }

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
on_all_down = "closed"
dns_ttl = "0s"
backends = ["10.1.3.1:5432", "10.1.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }

[services."ssh.svc.emfor"]
check = "tcp"
interval = "10s"
backends = ["10.1.0.11:22", "10.1.0.12:22"]
check_options = { expect = "SSH-" }
```

repository 裡附註解的範例設定 [examples/emfor.toml](../../examples/emfor.toml) 也列出了每一個 key。
