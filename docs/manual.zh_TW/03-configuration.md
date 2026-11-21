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
| `include_dir` | 無 | 一個目錄，裡面其他 `.toml` 檔會併進這份設定（[見下](#把設定拆到目錄)）。只能寫在主設定檔。必須寫在第一個 table 之前。 |

## [store]

```toml
[store]
path = "/var/lib/emfor/state.mdb"
map_size = "64MiB"
file_mode = "0640"
lock_file_mode = "0660"
sync = false
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `path` | `/var/lib/emfor/state.mdb` | store 檔案。client 也找這裡，除非 `EMFOR_STORE` 另有指定。 |
| `map_size` | `"64MiB"` | store 能成長到的最大大小；至少 `1MiB`，並無條件進位到 `64KiB` 的倍數（LMDB 要求是 page size 的倍數）。達到上限時 commit 會以 `MDB_MAP_FULL` 失敗。 |
| `file_mode` | `"0640"` | store 檔的權限，每次啟動都會設定，新檔與既有的檔都一樣。應用程式需要能讀取，所以它們的使用者要在 daemon 的群組裡；`"0644"` 讓每個本機使用者都能讀。 |
| `lock_file_mode` | `"0660"` | `<path>-lock` 的權限，每次啟動都會設定。應用程式需要能寫入，所以它們的使用者要在 daemon 的群組裡；`"0666"` 讓每個本機使用者都能寫，`emfor check-config` 與 daemon 啟動時會對其他人可寫的權限發出警告。見[誰能使用 store](08-operations.md#誰能使用-store)。0.7.0 起從 `"0644"` 與 `"0666"` 改成這兩個值。 |
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
| `zone` | `"svc.emfor"` | 只回答 zone 之下的名稱。最多 242 個字元。zone 之內的服務名稱要留出每個 backend 的 SRV 目標名稱空間：`<ip label>.<service>` 最多 253 個字元（IPv6 的 label 是 39 個字元）。 |
| `ttl` | `"1s"` | 回答與 zone SOA 的 TTL；`0s` 到 `3600s` 的整數秒。 |
| `rate_limit` | `100` | 每個 client 網段（IPv4 /24、IPv6 /56）每秒的 UDP 回應數；`0` 表示不限制。loopback 的 client 與 TCP 一律不受限。見[速率限制](07-dns.md#速率限制)。 |

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
| `members` | 必填 | 每個成員的 `listen`，包含這個 daemon 自己：2 到 64 個位址，不可重複，和 `listen` 一樣全是 IPv4 或全是 IPv6。同一個機房的每台主機可以用同一份清單。主機會來來去去的群組可以用腳本寫入：見[從雲端 API 取得 peers 成員](08-operations.md#從雲端-api-取得-peers-成員)。 |
| `secret_file` | 必填 | 共用密鑰的絕對路徑，用來驗證並加密每則訊息：檔案最多 65536 bytes（含結尾換行），去掉一個結尾換行後至少 32 bytes，其他使用者不能讀取、group 與其他人不能寫入（`chmod 600` 或 `640`），擁有者是 root 或執行 emfor 的使用者（group 不限）。 |
| `previous_secret_file` | 無 | 選用。`secret_file` 上次改變前那把密鑰的絕對路徑，條件與 `secret_file` 相同。只在啟動時讀取，讓更換密鑰期間重新啟動的 daemon 仍像沒有重新啟動一樣，用舊密鑰簽署並接受它（見下文）。 |
| `encrypt` | `true` | `true` 時每則訊息都加密送出，金鑰由密鑰導出：送給執行這個版本的成員用 `EMP3`（XChaCha20-Poly1305，24 bytes 隨機 nonce），送給其他成員用 `EMP2`（ChaCha20-Poly1305，12 bytes 隨機 nonce），依成員各自選擇，見下文。`false` 時只簽署（`EMP1`：HMAC-SHA256），網路上看得到 backend 名稱、位址與原因。`EMP2` 與 `EMP3` 一律接受；`EMP1` 只在 `accept_plaintext` 為 `true` 時接受，而且接受時若 `encrypt` 為 `true`，對仍送 `EMP1` 的每個成員，daemon 每分鐘記一行警告。 |
| `accept_plaintext` | `encrypt` 為 `true` 時 `false`，`encrypt = false` 時 `true` | `false` 時丟棄所有 `EMP1` 訊息（在 `emfor_peer_messages_rejected_total` 算成 `plaintext`），所以就算有人拿到密鑰，也無法讓這個 daemon 接受只簽署、未加密的訊息。因此用預設值時，會加密的 daemon 聽不到設了 `encrypt = false` 或執行 0.3.0 的成員：那個成員在這裡不算存活。群組裡還有成員送 `EMP1` 時，在這台主機設 `true`（見下文）；`false` 必須搭配 `encrypt = true`。0.7.0 以前，不論 `encrypt` 為何，預設都是 `true`。 |

只有當某個服務在兩邊的檢查定義相同時，成員之間才會分享該服務的回報：相同的 `check`、`check_options` 與 `timeout`。其他設定（`interval`、`rise`、`fall`、backend 清單）可以不同。

系統允許時（Linux 用 inotify，FreeBSD 與 macOS 用 kqueue）會監看密鑰檔：監看的是它所在的目錄，所以原地改寫，以及用新檔案 rename 蓋掉它，都會在發生時被看到（讀取還在進行中、或前 100 毫秒內才有另一個事件時，最晚在下一次 heartbeat 看到）；另外每 30 秒仍讀一次，防止事件漏掉；放在網路檔案系統上的檔案，其他主機所做的變更也是靠這次讀取才被看到。無法監看時（監看建立失敗，或系統沒有這種介面），daemon 記一行 `info`，改成每秒重新讀取一次。讀取很慢、或根本不回來（檔案在不回應的網路檔案系統上）時，heartbeat 不會因此停止：主機沿用最後有效的密鑰，等這次讀取結束才再讀，而且讀取執行超過 5 秒時記一行警告，最多每分鐘一次。要更換密鑰時，在一分鐘內把新密鑰寫到每台主機：主機自己的檔案改變後，仍用舊密鑰簽署 60 秒、接受舊密鑰 120 秒，所以期間不會丟失訊息。如果檔案在這 60 秒內又改了一次（例如修正寫錯的密鑰），主機會繼續用先前簽署所用的那把密鑰，簽到最後一次改變後 60 秒。

舊密鑰只保存在記憶體中，更換密鑰期間重新啟動的 daemon 只知道新密鑰：檔案還沒改的成員會丟棄它的訊息（`auth`），它也會丟棄那些成員的訊息，直到那些成員的檔案改變為止。若要在更換期間安全地重新啟動，把舊密鑰留在另一個檔案，並用 `previous_secret_file` 指向它，例如：

```sh
cp -p /etc/emfor/peers.key /etc/emfor/peers.key.old  # 保留權限
cat new.key > /etc/emfor/peers.key  # 每台主機寫入同一把新密鑰；權限不變
```

daemon 會在 store 旁邊的一個檔案記錄它用過的密鑰（`<store 路徑>-peers`，預設的 store 路徑下是 `state.mdb-peers`，權限 0600）：現行密鑰與被它換掉的密鑰的 SHA-256 指紋，以及換掉的時間；不會存密鑰本身。daemon 啟動時，若 `previous_secret_file` 裡的正是它在不到 120 秒前換掉的那把密鑰（執行中自己換的，或停機期間換的，後者依檔案的修改時間計），就會用那把密鑰簽署到該次改變後 60 秒，並接受它到該次改變後 120 秒。其他情況 `previous_secret_file` 一律無效，daemon 記一行 `info` 說明：更早的輪換已退役的密鑰、或 daemon 從未用過的密鑰，不會因為某個檔案裡有它、或 `secret_file` 被 touch 而再度被用，所以這個檔案可以一直留在設定裡。找不到記錄時（例如 0.7.0 第一次啟動）會忽略 `previous_secret_file`（下一次更換密鑰起才用）；記錄寫不出來時記一行警告，最多每分鐘一次，daemon 照常執行。

和接收端時鐘相差超過 5 秒的訊息會被丟棄，所以要讓主機時鐘保持同步（NTP）。

`encrypt = true` 時依成員選擇格式。這個版本送出的每則 `EMP2` 訊息，加密的 payload 裡都帶 `"emp3": true`；某個成員最近一則被接受的加密訊息是 `EMP3` 或這樣的 `EMP2` 時，送它 `EMP3`，否則送 `EMP2`：包括還沒接受過它的訊息時（啟動後、或 reload 加入它之後），以及它送來不帶這個欄位的 `EMP2`（0.4.0）或 `EMP1` 時。每次改變記一行 `info`（「sending EMP3 to peer ... from now on」）。加入 `EMP3` 是因為 12 bytes 隨機 nonce 在同一個密鑰下大約只能安全送 2^32 則訊息，大型群組幾天內就可能送到；24 bytes 的 nonce 沒有這個限制。0.4.0 的群組升級不需要改設定：一台一台升級即可，這個版本的成員彼此在第一則訊息後改用 `EMP3`，和 0.4.0 的成員之間用 `EMP2`。降回 0.4.0 也不需要改設定，`encrypt` 開或關都一樣：其他成員接受它的第一則訊息後就改回送 `EMP2`。在那之前（它啟動後約 1 秒），它會丟掉其他成員送來的 `EMP3`（算在 `malformed`）。

加密是 0.4.0 新增的。0.3.0 的成員只送、只收 `EMP1`，會丟棄加密的訊息（算在 `malformed`），所以從 0.3.0 升級群組時，每台主機升級時先設 `encrypt = false`；所有成員都升級後，刪掉這一行（或設 `encrypt = true`），再讓每個 daemon reload。從 0.7.0 起，`encrypt = true` 的 daemon 會丟棄還沒 reload 的成員送來的明文訊息，所以只要群組裡還有成員送明文，已經加密的主機就要明確設 `accept_plaintext = true`：把 0.3.0 或 0.4.0 的主機升級時，放在同一次設定變更裡（這兩個版本不認得這個鍵，它是 0.5.0 加入的）；0.5.0 或 0.6.0 的主機則可以在升級前先設。所有成員都已是 `encrypt = true`、`emfor_peer_messages_rejected_total{reason="plaintext"}` 也不再增加之後，刪掉 `accept_plaintext = true` 並 reload。加密沒有前向保密：日後取得密鑰的人可以讀出先前錄下的訊息。請照上面的方式定期更換密鑰，並讓密鑰檔只有 daemon 能讀。

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
| `interval` | `100ms` 到 `24h` | 一個 backend 的一次檢查結束到下一次開始的時間。 |
| `timeout` | 至少 `10ms`，不超過 `interval` | 一次檢查可以花多久。plugin 收到的是整數毫秒，並扣掉一小段餘裕。 |
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

**backend** 可以是字串；需要權重、site、`backup` 或 `drain` 時用 table：

```toml
backends = [
  "10.0.4.11:9200",
  { addr = "10.0.4.12:9200", weight = 3 },
  { addr = "10.1.4.11:9200", site = "dc2" },
  { addr = "10.2.4.11:9200", backup = true },
  { addr = "10.0.4.13:9200", drain = true },
]
```

| Key | 預設值 | 意義 |
| --- | --- | --- |
| `addr` | 必填 | `IPv4:port` 或 `[IPv6]:port`，port 為 1–65535。同一服務內不可重複。 |
| `weight` | 1 | 1 到 100。`balance = "weighted"` 與帶 affinity key 的 `"sticky"` 會用到。 |
| `site` | 無 | backend 所在的資料中心。`balance = "locality"` 會用到。 |
| `backup` | `false` | `true` 或 `false`。備援 backend 只在沒有其他 backend 通過檢查時使用（[第 5 章](05-backend-selection.md#備援-backend)）。每個服務至少要有一個不是備援的 backend。 |
| `drain` | `false` | `true` 或 `false`。drain 的 backend 任何 `balance` 與 DNS 都不會選它，但檢查照跑（[第 5 章](05-backend-selection.md#drain-一個-backend)）。 |

任何 `balance` 都接受 `weight`、`site`、`backup` 與 `drain`，所以切換 `balance` 時不必修改 backend。

## [plugins."name"]

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-check", "redis"]
```

| Key | 意義 |
| --- | --- |
| `command` | 必填。程式與它的參數；第一個元素是可執行檔的絕對路徑。任何元素都不能含 NUL。不經過 shell 執行。 |

名稱為 1–63 個 `a-z`、`0-9`、`_` 與 `-` 字元，且不可為 `tcp`、`http` 或 `exec`。服務以 `check = "<name>"` 使用 plugin。每個宣告的 plugin 都會被啟動，不論有沒有服務使用。見[第 4 章](04-health-checks.md#plugin-與-emfor-check)。

## 把設定拆到目錄

主設定檔裡寫 `include_dir = "conf.d"`，daemon 會先讀主設定檔，再讀該目錄裡每個 `.toml` 檔，檢查之前先合併成一份設定。相對路徑相對於主設定檔所在的目錄（`/etc/emfor/emfor.toml` 對應 `/etc/emfor/conf.d`）。這樣套件可以管 `emfor.toml`，各團隊或設定管理工具則各管自己服務的檔案：

```
/etc/emfor/emfor.toml            include_dir = "conf.d"、[store]、[dns]
/etc/emfor/conf.d/01-plugins.toml
/etc/emfor/conf.d/10-web.toml
/etc/emfor/conf.d/99-local.toml
```

**讀哪些檔、什麼順序。** 名稱以 `.toml` 結尾、不以 `.` 開頭的一般檔案，以及指向一般檔案的連結。依檔名的位元組順序讀取，不看地區語言的排序；後面的檔案蓋掉前面的，主設定檔是最底層。數字請補零：`9-a.toml` 排在 `10-a.toml` 之後。其他東西一律忽略（`web.toml~`、`.web.toml.swp`、`web.toml.disabled`、子目錄），所以把檔案改名就能讓它退出設定。檔名不是 UTF-8 的會略過並在 log 裡警告。空目錄沒有問題。

**怎麼合併。** 兩個 table 逐鍵合併，有多深合多深。其他一律由後面的值整個取代：數字、字串、清單（包含 `backends` 與 `command`），或型別不同的值。

```toml
# emfor.toml
[services."api.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80", "10.0.0.2:80"]
interval = "2s"
```

```toml
# conf.d/99-local.toml
[services."api.svc.emfor"]
interval = "5s"
```

這個服務保有原來的 `check` 與兩個 `backends`，`interval` 變成 `"5s"`。要改 backend，就把整份清單再寫一次。後面的檔案不能刪掉服務、plugin 或 backend；要從定義它的檔案裡拿掉，或把那個檔案改名讓它被忽略。合併後沒有 `check` 或沒有 `backends` 的服務是錯誤，和單一檔案一樣。合併後的設定，其檢查、啟動、reload 與跟 peer 的交換，都和把同樣內容寫在單一檔案裡完全一樣；怎麼拆檔不會改變 peer 比對的服務定義。

**只能放在主設定檔的鍵。** `[store]` 與 `include_dir` 本身。`emfor status` 與 `emfor lookup` 只讀主設定檔，所以片段不能搬動 store，也不能再引入別的目錄。只用 `status` 與 `lookup` 的人只要能讀主設定檔，不需要讀那個目錄。其他東西，包含 `site`、`[dns]`、`[peers]`、`[plugins]`，放在哪個檔案都可以。

**權限。** 目錄與裡面被讀的每個檔案都不能讓 group 或其他人寫入（`0755` 與 `0644` 可以，`0775` 與 `0664` 不行），否則整份設定被拒絕，錯誤會指出路徑與權限。擁有者必須是 root 或執行 emfor 的使用者（符號連結看它指向的檔案的擁有者），否則同樣被拒絕，因為其他人擁有的檔案，那個人可以隨時改。請以 daemon 執行的使用者身分跑 `emfor check-config`（systemd unit 與 FreeBSD 的 `configtest` 就是這樣）；手動以 root 執行時，只接受 root 擁有的檔案。片段可以宣告 plugin 的 `command` 或 `exec` 檢查，能寫那裡的人就能以 daemon 的使用者身分執行程式。主設定檔也同樣被檢查：`emfor server`（啟動與每次 reload）和 `emfor check-config` 讀它的時候，因為它同樣能宣告 plugin 的 `command`。錯誤訊息還會給出修正指令：權限不對是 `chmod g-w,o-w /etc/emfor/emfor.toml`，擁有者不對是 `chown root /etc/emfor/emfor.toml`。`emfor status` 與 `emfor lookup` 只讀主設定檔的 `[store]`，不檢查。主設定檔所在的目錄（`/etc/emfor`）不檢查：請像 deb 與 rpm 套件那樣，只讓 root 能寫。0.7.0 之前主設定檔不被檢查；見[升級](08-operations.md#升級)。

**目錄必須存在。** 套件會建立它。`include_dir` 指向不存在的目錄是錯誤：啟動時 daemon 記錄錯誤並以 2 結束；reload 時這次 reload 被拒絕，舊設定繼續執行。

**錯誤會指出檔案。** TOML 語法錯誤以 `檔案:行:欄` 回報，其他錯誤是 TOML 路徑加上設定該鍵的檔案（`services."api.svc.emfor".interval: ... (in /etc/emfor/conf.d/99-local.toml)`）。錯誤來自兩個檔案的值時（例如一個檔案的 `timeout` 與另一個檔案的 `interval`），指出的是比較後面的那個檔案。

**寫檔的方式。** 先寫成目錄外或不以 `.toml` 結尾的暫存檔名，再改名放進去。reload 讀到寫到一半的檔案會被拒絕，舊設定繼續執行，但用原子改名可以避免這次失敗的 reload。

## 檢查設定檔

```sh
$ emfor check-config
config ok: 8 services, 19 backends
```

`emfor check-config` 檢查整個檔案並印出每一個錯誤，每個錯誤都帶著它的 TOML 路徑，例如：

```
services."api.svc.emfor".timeout: timeout must not exceed interval
```

設定了 `include_dir` 時，它接著列出讀過的檔案，主設定檔在前，其他依合併順序排列：

```
config ok: 8 services, 19 backends
read /etc/emfor/emfor.toml
read /etc/emfor/conf.d/10-web.toml
read /etc/emfor/conf.d/99-local.toml
```

daemon 在啟動與每次 reload 之後記錄同一份清單。沒有 `include_dir` 時輸出只有那一行。

**看合併後的設定。** `emfor check-config --show` 以同樣的方式檢查設定，接著把合併後的設定以 TOML 印到標準輸出，每個值後面寫明最後設定它的檔案：

```
# include_dir = "conf.d" (its files are merged below)

[services."api.svc.emfor"]
backends = ["10.0.0.1:80", "10.0.0.2:80"] # /etc/emfor/emfor.toml
check = "tcp" # /etc/emfor/emfor.toml
interval = "5s" # /etc/emfor/conf.d/99-local.toml
```

`backends` 這類清單是整個一起設定的，所以它的註解寫的是設定這份清單的檔案。`config ok` 與 `read` 行會寫到標準錯誤，所以 `emfor check-config --show > merged.toml` 存下來的檔案在哪裡都可以再被檢查，因為 `include_dir` 那一行只以註解寫出（它的檔案已經合併進來了）；除了註解，內容相同。設定裡除了 `include_dir` 之外的相對路徑，都不依設定檔所在的目錄而變。無效的設定在標準輸出不印任何東西，並以 2 結束。沒有 `include_dir` 時，輸出開頭有一行註解寫明檔案，沒有其他註解。內建預設值不會加進去：你沒寫的鍵不會出現。沒有任何值被遮蔽；設定裡放的是密碼檔的路徑，不是密碼，但 plugin 的 `command` 會照原樣印出，所以不要把祕密寫在那裡。註解的格式是給人看的，不要用程式解析它。

設定有效時結束代碼為 0，否則為 2（輸出寫不出去時，例如磁碟滿了，為 1；管線被關掉不算錯誤）。它會啟動宣告的 plugin，讓它們檢查各服務的 `check_options`，並在結束前停止它們。它不會開啟 store。以無效設定啟動的 daemon 會記錄同樣的錯誤，並以 2 結束。

## reload 會改變什麼

`SIGHUP`（`systemctl reload emfor`、`service emfor reload`）會重新讀取檔案，連同 `include_dir` 裡的檔案。只要有任何錯誤（不論在哪個檔案），或 `include_dir` 不見了，整份設定就被拒絕：daemon 記錄每一個錯誤，繼續用舊的設定執行。

| 變更 | reload 時 |
| --- | --- |
| 新增或移除服務與 backend | 套用；新的 backend 從 `unknown` 開始。 |
| 服務的 `check` 或 `check_options` | 套用；它的 backend 從 `unknown` 重新開始。 |
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`peers` | 套用；backend 保留原狀態。新的 `interval` 或 `jitter` 也套用到正在進行的等待：下一次檢查改在上一次結束後經過新的等待時間時開始，若那個時間已經過了就立刻開始。 |
| `balance`、`weight`、`site`（最上層與 backend）、`backup`、`drain`、`dns_ttl`、`slow_start` | 套用；backend 保留狀態，正在慢啟動的 backend 保留它的恢復時間。 |
| `[dns] zone`、`[dns] ttl`、`[dns] rate_limit` | 套用。 |
| `[log] level` | 套用（除非設定了 `RUST_LOG`）。 |
| plugin 的 `command` | 該 plugin 重新啟動，使用它的服務從 `unknown` 重新開始。未變更的 plugin 繼續執行。 |
| `[store]` 中的任何設定 | 拒絕；需要重新啟動。 |
| `[dns] listen`、新增或移除 `[dns]` | 拒絕（`dns listen settings require restart`）。 |
| `[metrics] listen`、新增或移除 `[metrics]` | 拒絕（`metrics listen settings require restart`）。 |
| `[peers] members`、`[peers] secret_file`、`[peers] encrypt`、`[peers] accept_plaintext` | 套用；新加入的成員從不存活開始。`previous_secret_file` 會檢查，但只在啟動時讀取。 |
| `[peers] listen`、新增或移除 `[peers]` | 拒絕（`peers listen settings require restart`）。 |
