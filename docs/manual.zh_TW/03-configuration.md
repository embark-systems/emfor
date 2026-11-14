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
| `site` | 無 | 這台主機所在的資料中心。`balance = "locality"` 會用到，有 `[peers]` 時必填。1–63 個 `a-z`、`0-9` 與 `-` 字元（讀取時轉為小寫）。必須寫在第一個 table 之前。 |

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

## [peers]

選用。讓這個 daemon 成為**聯防群組**的成員：同一個 `site`、在這裡列出彼此的 emfor daemon。成員每秒互送一次 heartbeat；其中一台看到 backend 從 `healthy` 變成 `unhealthy` 時通知其他成員，它們立刻重新檢查那個 backend，之後每 200ms 一次（最多 `fall` 次），而不是等到下一個 `interval`。存活成員有三台以上時，每個 backend 有三台負責主機：它們每個 `interval` 檢查一次，並把結果送給其他成員；三台中有兩台回報 `down`，每個成員都把該 backend 標成 down。其他成員讓兩個一致的結果決定，大約每三個 `interval` 才自己檢查一次；其他成員認為正常、本機檢查卻失敗時，每 200ms 重新檢查一次。投票與分工沒有自己的設定，使用各服務的 `interval`，而且只計入 `check`、`check_options` 與 `timeout` 相同的成員。client 仍然只讀本機的 store。沒有 `[peers]` 時以上都不會執行。見[第 1 章](01-concepts.md#聯防群組)。

設了 `peers = false` 的服務（寫在服務的 table 或 `[defaults]`）在這台主機上不參加群組：不送通知與結果、忽略收到的通知與結果，一律依自己的檢查判斷。其他服務仍在群組裡。這台主機會在 heartbeat 裡宣告它不參加的服務（服務名稱的短雜湊，最多 80 個），其他成員就不把它算成這些服務的負責主機；只要存活成員夠多，這些服務的 backend 仍有三台負責主機。因此每台主機可以各自決定哪些服務不參加。聯防群組與這項宣告都是 0.3.0 加入的，而 0.2.0 不接受有 `[peers]` 的設定，所以每個成員都會讀這項宣告。不參加的服務超過 80 個時，這台主機會記一行警告，其他成員對沒宣告到的服務仍會把它算成候選。

```toml
site = "dc1"                               # 有 [peers] 時必填

[peers]
listen = "10.0.0.5:5302"
members = ["10.0.0.5:5302", "10.0.0.6:5302", "10.0.0.7:5302"]
secret_file = "/etc/emfor/peers.key"
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `listen` | 必填 | 這個 daemon 在群組中的 UDP 位址，必須是 `members` 之一。要寫具體位址，不能是 `0.0.0.0` 或 `::`；其他成員會檢查訊息是否來自這個位址。慣用的 port 是 5302。 |
| `members` | 必填 | 每個成員的 `listen`，包含這個 daemon 自己：2 到 64 個位址，不可重複，和 `listen` 一樣全是 IPv4 或全是 IPv6。同一個機房的每台主機可以用同一份清單。 |
| `secret_file` | 必填 | 共用密鑰的絕對路徑，用來驗證並加密每則訊息：去掉一個結尾換行後至少 32 bytes，其他使用者不能讀取（`chmod 600` 或 `640`）。 |
| `encrypt` | `true` | `true` 時每則訊息都加密送出（`EMP2`：ChaCha20-Poly1305，金鑰由密鑰導出）；`false` 時只簽署（`EMP1`：HMAC-SHA256），網路上看得到 backend 名稱、位址與原因。兩種格式都一律接受；`true` 時，對仍送 `EMP1` 的每個成員，daemon 每分鐘記一行警告。 |

只有當某個服務在兩邊的檢查定義相同時，成員之間才會分享該服務的回報：相同的 `check`、`check_options` 與 `timeout`。其他設定（`interval`、`rise`、`fall`、backend 清單）可以不同。

密鑰檔每秒重新讀取一次。要更換密鑰時，在一分鐘內把新密鑰寫到每台主機：主機自己的檔案改變後，仍用舊密鑰簽署 60 秒、接受舊密鑰 120 秒，所以期間不會丟失訊息。和接收端時鐘相差超過 5 秒的訊息會被丟棄，所以要讓主機時鐘保持同步（NTP）。

加密是這個版本新增的。0.3.0 的成員只送、只收 `EMP1`，會丟棄加密的訊息（算在 `malformed`），所以升級群組時，每台主機升級時先設 `encrypt = false`；所有成員都升級後，刪掉這一行（或設 `encrypt = true`），再讓每個 daemon reload。加密沒有前向保密：日後取得密鑰的人可以讀出先前錄下的訊息。請照上面的方式定期更換密鑰，並讓密鑰檔只有 daemon 能讀。

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
peers = true
slow_start = "0s"
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
| `peers` | `true` 或 `false` | 這個服務是否參加[聯防群組](#peers)。沒有 `[peers]` 時沒有作用。 |
| `slow_start` | `0s`（關閉），或 `1s` 到 `1h` | 恢復的 backend 在這段時間內逐步增加分到的流量（[第 5 章](05-backend-selection.md#慢啟動slow-start)）。 |

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
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`balance`、`peers`、`slow_start` | 否 | 為這個服務覆寫 `[defaults]`。 |
| `dns_ttl` | 否 | 為這個服務覆寫 `[dns] ttl`；整數秒，`0s` 到 `3600s`。 |

**服務名稱**類似主機名稱：轉為小寫、去掉一個結尾的點，最多 253 個字元，由點分隔的 label 組成，每個 label 1–63 個 `a-z`、`0-9` 與 `-` 字元，且不以 `-` 開頭或結尾。只差在大小寫的兩個名稱視為重複。client 查詢時也用同樣方式正規化名稱。

**backend** 可以是字串；需要權重、site 或 `backup` 時用 table：

```toml
backends = [
  "10.0.4.11:9200",
  { addr = "10.0.4.12:9200", weight = 3 },
  { addr = "10.1.4.11:9200", site = "dc2" },
  { addr = "10.2.4.11:9200", backup = true },
]
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `addr` | 必填 | `IPv4:port` 或 `[IPv6]:port`，port 為 1–65535。同一服務內不可重複。 |
| `weight` | 1 | 1 到 100。`balance = "weighted"` 與帶 affinity key 的 `"sticky"` 會用到。 |
| `site` | 無 | backend 所在的資料中心。`balance = "locality"` 會用到。 |
| `backup` | `false` | `true` 或 `false`。備援 backend 只在沒有其他 backend 通過檢查時使用（[第 5 章](05-backend-selection.md#備援-backend)）。每個服務至少要有一個不是備援的 backend。 |

任何 `balance` 都接受 `weight`、`site` 與 `backup`，所以切換 `balance` 時不必修改 backend。

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
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`peers` | 套用；backend 保留原狀態。 |
| `balance`、`weight`、`site`（最上層與 backend）、`backup`、`dns_ttl`、`slow_start` | 套用；backend 保留狀態，正在慢啟動的 backend 保留它的恢復時間。 |
| `[dns] zone`、`[dns] ttl` | 套用。 |
| `[log] level` | 套用（除非設定了 `RUST_LOG`）。 |
| plugin 的 `command` | 該 plugin 重新啟動，使用它的服務從 `unknown` 重新開始。未變更的 plugin 繼續執行。 |
| `[store]` 中的任何設定 | 拒絕；需要重新啟動。 |
| `[dns] listen`、新增或移除 `[dns]` | 拒絕（`dns listen settings require restart`）。 |
| `[metrics] listen`、新增或移除 `[metrics]` | 拒絕（`metrics listen settings require restart`）。 |
| `[peers] members`、`[peers] secret_file`、`[peers] encrypt` | 套用；新加入的成員從不存活開始。 |
| `[peers] listen`、新增或移除 `[peers]` | 拒絕（`peers listen settings require restart`）。 |
