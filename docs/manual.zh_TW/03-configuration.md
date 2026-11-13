# 3. 設定

[手冊](README.md) · 上一章：[2. 安裝](02-installation.md) · 下一章：[4. 健康檢查](04-health-checks.md)

daemon 讀取一個 TOML 檔，預設為 `/etc/emfor/emfor.toml`（[第 2 章](02-installation.md#預設設定檔路徑)）。除了服務的 `check` 與 `backends`，每個 key 都是選用的。[examples/emfor.toml](../../examples/emfor.toml) 附有註解，列出所有選項。

## 值的格式

| 種類 | 格式 | 範例 |
| --- | --- | --- |
| 時間長度 | 帶單位的字串 | `"500ms"`、`"2s"`、`"1m"` |
| 大小 | 帶單位的字串 | `"64MiB"` |
| 檔案權限 | 八進位字串 | `"0644"` |
| 百分比 | 以 `%` 結尾的字串 | `"10%"` |
| 位址 | `IPv4:port` 或 `[IPv6]:port` | `"10.0.0.11:8080"`、`"[2001:db8::11]:8080"` |

檔案中任何位置出現未知的 key 都是錯誤，所以拼錯的 key 一定會被發現。

## 最上層

```toml
site = "dc1"
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `site` | 無 | 這台主機所在的資料中心。`balance = "locality"` 會用到。1–63 個 `a-z`、`0-9` 與 `-` 字元（讀取時轉為小寫）。必須寫在第一個 table 之前。 |

## [store]

```toml
[store]
path = "/var/lib/emfor/state.mdb"
map_size = "64MiB"
file_mode = "0644"
lock_file_mode = "0666"
sync = false
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `path` | `/var/lib/emfor/state.mdb` | store 檔案。client 也找這裡，除非 `EMFOR_STORE` 另有指定。 |
| `map_size` | `"64MiB"` | store 能成長到的最大大小；至少 `1MiB`。達到上限時 commit 會以 `MDB_MAP_FULL` 失敗。 |
| `file_mode` | `"0644"` | 建立 store 檔案時的權限。應用程式需要能讀取。 |
| `lock_file_mode` | `"0666"` | `<path>-lock` 的權限。應用程式需要能寫入。保留預設值前請先看[誰能使用 store](08-operations.md#誰能使用-store)。 |
| `sync` | `false` | `true` 會在每次 commit 時寫入磁碟。見[磁碟上的 store](08-operations.md#磁碟上的-store)。 |

## [log]

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `level` | `"info"` | `trace`、`debug`、`info`、`warn` 或 `error`。設定了環境變數 `RUST_LOG` 時，整次執行都以它為準。 |

## [dns]

只有存在這個 table 時才會啟動 DNS server；空的 `[dns]` table 以預設值啟動。見[第 7 章](07-dns.md)。

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5300"]` | 回答查詢的位址，UDP 與 TCP。 |
| `zone` | `"svc.emfor"` | 只回答 zone 之下的名稱。最多 242 個字元。 |
| `ttl` | `"1s"` | 回答與 zone SOA 的 TTL；`0s` 到 `3600s` 的整數秒。 |

## [metrics]

只有存在這個 table 時才會啟動 Prometheus endpoint。

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5301"]` | 提供 `/metrics` 的位址。 |

## [defaults]

每個服務沒有自行設定時使用的值。內建值如下：

```toml
[defaults]
interval = "2s"
timeout = "1s"
rise = 2
fall = 3
jitter = "10%"
on_all_down = "open"
balance = "round-robin"
```

| Key | 規則 | 意義 |
| --- | --- | --- |
| `interval` | 至少 `100ms` | 一個 backend 的一次檢查結束到下一次開始的時間。 |
| `timeout` | 大於 0，不超過 `interval` | 一次檢查可以花多久。 |
| `rise` | 至少 1 | 連續通過幾次後 backend 變成 healthy。 |
| `fall` | 至少 1 | 連續失敗幾次後 backend 變成 unhealthy。 |
| `jitter` | `0%` 到 `50%` | `interval` 的隨機浮動，讓檢查不會擠在同一時刻。 |
| `on_all_down` | `open` 或 `closed` | 沒有 backend 健康時查詢怎麼做（[第 5 章](05-backend-selection.md#沒有任何-backend-健康時)）。 |
| `balance` | 見[第 5 章](05-backend-selection.md) | 如何選擇 backend。 |

## [services."name"]

```toml
[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
interval = "1s"
dns_ttl = "5s"

[services."api.svc.emfor".check_options]
path = "/healthz"
```

| Key | 必填 | 意義 |
| --- | --- | --- |
| `check` | 是 | `tcp`、`http`、`exec`，或已宣告的 plugin 名稱。 |
| `backends` | 是 | 非空的 backend 清單（見下）。 |
| `check_options` | 否 | checker 的選項（[第 4 章](04-health-checks.md)）。 |
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`balance` | 否 | 為這個服務覆寫 `[defaults]`。 |
| `dns_ttl` | 否 | 為這個服務覆寫 `[dns] ttl`；整數秒，`0s` 到 `3600s`。 |

**服務名稱**類似主機名稱：轉為小寫、去掉一個結尾的點，最多 253 個字元，由點分隔的 label 組成，每個 label 1–63 個 `a-z`、`0-9` 與 `-` 字元，且不以 `-` 開頭或結尾。只差在大小寫的兩個名稱視為重複。client 查詢時也用同樣方式正規化名稱。

**backend** 可以是字串；需要權重或 site 時用 table：

```toml
backends = [
  "10.0.4.11:9200",
  { addr = "10.0.4.12:9200", weight = 3 },
  { addr = "10.1.4.11:9200", site = "dc2" },
]
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `addr` | 必填 | `IPv4:port` 或 `[IPv6]:port`，port 為 1–65535。同一服務內不可重複。 |
| `weight` | 1 | 1 到 100。`balance = "weighted"` 會用到。 |
| `site` | 無 | backend 所在的資料中心。`balance = "locality"` 會用到。 |

任何 `balance` 都接受 `weight` 與 `site`，所以切換 `balance` 時不必修改 backend。

## [plugins."name"]

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-check", "redis"]
```

| Key | 意義 |
| --- | --- |
| `command` | 必填。程式與它的參數；第一個元素是可執行檔的絕對路徑。不經過 shell 執行。 |

名稱為 1–63 個 `a-z`、`0-9`、`_` 與 `-` 字元，且不可為 `tcp`、`http` 或 `exec`。服務以 `check = "<name>"` 使用 plugin。每個宣告的 plugin 都會被啟動，不論有沒有服務使用。見[第 4 章](04-health-checks.md#plugin-與-emfor-check)。

## 檢查設定檔

```sh
$ emfor check-config
config ok: 8 services, 19 backends
```

`emfor check-config` 檢查整個檔案並印出每一個錯誤，每個錯誤都帶著它的 TOML 路徑，例如：

```
services."api.svc.emfor".timeout: timeout must not exceed interval
```

設定有效時結束代碼為 0，否則為 2。它會啟動宣告的 plugin，讓它們檢查各服務的 `check_options`，並在結束前停止它們。它不會開啟 store。以無效設定啟動的 daemon 會記錄同樣的錯誤，並以 2 結束。

## reload 會改變什麼

`SIGHUP`（`systemctl reload emfor`、`service emfor reload`）會重新讀取檔案。只要有任何錯誤，整份設定就被拒絕：daemon 記錄每一個錯誤，繼續用舊的設定執行。

| 變更 | reload 時 |
| --- | --- |
| 新增或移除服務與 backend | 套用；新的 backend 從 `unknown` 開始。 |
| 服務的 `check` 或 `check_options` | 套用；它的 backend 從 `unknown` 重新開始。 |
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down` | 套用；backend 保留原狀態。 |
| `balance`、`weight`、`site`（最上層與 backend）、`dns_ttl` | 套用。 |
| `[dns] zone`、`[dns] ttl` | 套用。 |
| `[log] level` | 套用（除非設定了 `RUST_LOG`）。 |
| plugin 的 `command` | 該 plugin 重新啟動，使用它的服務從 `unknown` 重新開始。未變更的 plugin 繼續執行。 |
| `[store]` 中的任何設定 | 拒絕；需要重新啟動。 |
| `[dns] listen`、新增或移除 `[dns]` | 拒絕（`dns listen settings require restart`）。 |
| `[metrics] listen`、新增或移除 `[metrics]` | 拒絕（`metrics listen settings require restart`）。 |
