# 5. 設定 emfor

[手冊](README.md) · 上一章：[4. 安裝](04-installing.md) · 下一章：[6. 健康檢查](06-health-checks.md)

這一章談整體的設定：它的格式、如何拆成多個檔案、如何檢查，以及如何套用變更。檢查在[第 6 章](06-health-checks.md)與[第 7 章](07-plugins.md)，backend 的選擇在[第 8 章](08-choosing-a-backend.md)，每個 key 與它的預設值在[第 18 章](18-config-reference.md)。

## 設定檔的樣子

設定是一個 TOML 檔，預設為 `/etc/emfor/emfor.toml`。只有服務的 `check` 與 `backends` 是必填；其他都有預設值。一份實際的小型設定：

```toml
site = "dc1"                       # 這台主機的資料中心（選用）

[log]
level = "info"

[dns]                              # 選用的 DNS server，使用預設值
[metrics]                          # 選用的 /metrics，使用預設值

[defaults]                         # 套用到每個服務
interval = "2s"
timeout = "1s"

[plugins."pgsql"]
command = ["/usr/bin/emfor-check", "pgsql"]

[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
check_options = { path = "/healthz" }

[services."db.svc.emfor"]
check = "pgsql"
balance = "failover"
on_all_down = "closed"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
check_options = { user = "emfor", database = "postgres", role = "primary", password_file = "/etc/emfor/secrets/pg.pass" }
```

各個 table：

| Table | 內容 |
| --- | --- |
| 最上層 | `site` 與 `include_dir`。它們必須寫在第一個 `[table]` 之前，因為 TOML 會把 table 標頭之後的 key 歸進那個 table。 |
| `[store]` | state store 的位置、大小與檔案權限。 |
| `[log]` | log 等級。 |
| `[dns]` | DNS server；只有存在這個 table 時才會執行。 |
| `[metrics]` | Prometheus endpoint；只有存在這個 table 時才會執行。 |
| `[peers]` | 聯防群組（[第 11 章](11-peer-groups.md)）。 |
| `[defaults]` | 每個服務沒有自行設定時使用的設定。 |
| `[plugins."<name>"]` | 一個 plugin 程式，服務可以把它當作自己的 `check`。 |
| `[services."<name>"]` | 一個服務：它的檢查、backend 與設定。 |

檔案中任何位置出現**未知的 key 都是錯誤**，所以拼錯的 key 一定會被發現。反過來說：較新版本加入的 key，會讓較舊的版本拒絕這份設定。

## 值的格式

| 種類 | 格式 | 範例 |
| --- | --- | --- |
| 時間長度 | 字串：一個或多個「數字加單位」。單位為 `ms`、`s`、`m`、`h`、`d`，以及拼出的完整形式（`seconds`、`minutes`……）；可以有小數；單獨的 `"0"` 表示零。 | `"500ms"`、`"2s"`、`"1.5s"`、`"1h 30m"`、`"2 seconds"` |
| 大小 | 帶 `KiB`、`MiB` 或 `GiB` 的字串，或單純的位元組數 | `"64MiB"` |
| 檔案權限 | 八進位字串 | `"0640"` |
| 百分比 | 以 `%` 結尾的字串 | `"10%"` |
| 位址 | `IPv4:port` 或 `[IPv6]:port`，port 為 1–65535 | `"10.0.0.11:8080"`、`"[2001:db8::11]:8080"` |

單純的數字不是時間長度：`interval = "2"` 會被拒絕，訊息為 `invalid duration "2", use e.g. "500ms" or "2s"`。

## 設定與預設值

每個服務的時間與選擇設定依序取自：它自己的 table、`[defaults]`，最後是內建值：

```toml
[defaults]
interval = "2s"          # 一次檢查結束到下一次開始的時間
timeout = "1s"           # 一次檢查可以花多久；最多為 interval
rise = 2                 # 連續通過幾次後變成 healthy
fall = 3                 # 連續失敗幾次後變成 unhealthy
jitter = "10%"           # interval 的隨機浮動，0% 到 50%
on_all_down = "open"     # 或 "closed"
balance = "round-robin"
peers = true             # 有聯防群組時參加
slow_start = "0s"        # 關閉；或 1s 到 1h
```

這正是內建的那一組值，所以這個 table 不改變任何東西。服務可以覆寫其中任何一項：

```toml
[services."report.svc.emfor"]
check = "tcp"
backends = ["10.0.9.1:5432"]
interval = "30s"
timeout = "5s"
fall = 2
```

## 把設定拆到目錄

設定 `include_dir` 後，daemon 先讀主設定檔，再讀一個目錄裡的每個 `.toml` 檔，合併成一份設定。這樣套件可以管主設定檔，各團隊或工具則各管自己服務的檔案：

```
/etc/emfor/emfor.toml              include_dir = "conf.d", [dns]
/etc/emfor/conf.d/10-web.toml
/etc/emfor/conf.d/99-local.toml
```

```toml
# /etc/emfor/emfor.toml
include_dir = "conf.d"        # 相對於這個檔案所在的目錄

[dns]
```

deb 與 rpm 套件附的就是這種配置，主設定檔裡只有 `include_dir` 那一行。

**讀哪些檔、什麼順序。**

- 名稱以 `.toml` 結尾、不以 `.` 開頭的一般檔案，以及指向一般檔案的連結。其他一律忽略：`web.toml~`、`.web.toml.swp`、`web.toml.disabled`、子目錄。把檔案改名成 `.disabled` 就能讓它退出設定。
- 依檔名的位元組順序讀取，不看你的地區語言排序。數字請補零：`9-a.toml` 排在 `10-a.toml` 之後。
- 主設定檔是最底層；後面的檔案蓋掉前面的。
- 空目錄沒有問題。目錄不存在是錯誤。

**怎麼合併。** 兩個 table 逐鍵合併，有多深合多深。其他東西（數字、字串、清單）一律由後面的值整個取代。所以：

```toml
# /etc/emfor/conf.d/10-web.toml
[services."web.svc.emfor"]
check = "tcp"
backends = ["10.0.0.21:80", "10.0.0.22:80"]
```

```toml
# /etc/emfor/conf.d/99-local.toml
[services."web.svc.emfor"]
interval = "5s"
```

`web.svc.emfor` 保有它的 `check` 與兩個 backend，並得到 `interval = "5s"`。要改 backend，就把整份清單再寫一次。後面的檔案不能刪掉服務、plugin 或 backend：要從定義它的檔案裡拿掉。

**只能放在主設定檔**：`[store]` 與 `include_dir` 本身。設定它們的片段會被拒絕（`/etc/emfor/conf.d/60-s.toml: store is only allowed in the main config file`）。其他東西，包含 `site`、`[dns]`、`[peers]` 與 `[plugins]`，放在哪個檔案都可以。

**以原子方式寫檔。** 先用不以 `.toml` 結尾的名稱（或在目錄外）寫出新檔，再用 `mv` 放到定位。reload 讀到寫到一半的檔案會被拒絕，舊設定繼續執行，但用改名可以避免這次失敗的 reload。

## 誰可以擁有、寫入設定檔

設定可以啟動程式（plugin 的 `command`、`exec` 檢查），這些程式以 daemon 的使用者身分執行。所以 emfor 拒絕別人可能改動的設定檔：

- **主設定檔**、**`include_dir` 目錄**與**其中每個檔案**都不能讓 group 或其他人寫入：權限 `0644` 與 `0755` 可以，`0664` 與 `0775` 不行。
- 它們的**擁有者必須是 root 或 daemon 執行時的使用者**。連結看的是它指向的檔案。

```console
# chmod 0664 /etc/emfor/emfor.toml
# emfor check-config
/etc/emfor/emfor.toml: writable by group or others (mode 0664); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/emfor.toml
```

只要 `emfor server`（啟動時與每次 reload）和 `emfor check-config` 讀取設定，就會做這項檢查。0.7.0 之前只檢查片段，不檢查主設定檔。`emfor status` 與 `emfor lookup` 只讀主設定檔的 `[store]` table，不做檢查。主設定檔所在的目錄（`/etc/emfor`）不檢查；請像套件那樣，只讓 root 能寫入。

擁有者的規則取決於誰執行指令。請以 daemon 的使用者身分執行 `emfor check-config`，systemd unit 與 FreeBSD 的 `configtest` 就是這樣做的。以 root 手動執行時，只接受 root 擁有的檔案。

## 檢查設定

對本章開頭的設定：

```console
# emfor check-config
service db.svc.emfor: check_options has a password_file but tls is not true: the password is not protected on the network, and anyone who can see the traffic, or answer in the backend's place, can get it
config ok: 2 services, 4 backends
```

`emfor check-config` 讀取整份設定並印出**每一個**錯誤，每個錯誤都帶著它的 TOML 路徑，設定拆成多個檔案時還帶著檔案。這裡上面拆開的範例多了一個檔案 `conf.d/50-x.toml`，它為 `web.svc.emfor` 設定 `timeout = "10s"`：

```console
# emfor check-config
services."web.svc.emfor".timeout: timeout must not exceed interval (in /etc/emfor/conf.d/50-x.toml)
```

錯誤來自兩個檔案的值時（一個檔案的 `timeout`，另一個檔案的 `interval`），指出的是最後設定錯誤路徑中那個 key（這裡是 `timeout`）的檔案，即使 `99-local.toml` 比它更晚設定 `interval`。沒有任何檔案設定那個 key 本身時，指出的是最後設定它上層最近的 table 的檔案。

它做的事：

- 啟動每個宣告的 plugin，讓它檢查使用它的服務的 `check_options`，然後停止它。所以錯誤的 plugin 選項在你 reload 之前就會被發現。
- 像檢查時一樣讀取密碼檔、CA 檔與 peer 密鑰檔。
- 對其他使用者讀得到的密鑰檔、未經 TLS 送出的密碼，以及其他使用者能寫入的 store 檔案權限發出警告（不會因此失敗）（[第 13 章](13-security.md)）。
- 不開啟 store，也不需要 daemon 在執行。
- 設定有效時結束代碼為 0，無效時為 2，輸出寫不出去時為 1（磁碟滿了；管線被關掉不算錯誤）。

設定了 `include_dir` 時，它接著依合併順序列出讀過的檔案：

```console
# emfor check-config
config ok: 1 services, 2 backends
read /etc/emfor/emfor.toml
read /etc/emfor/conf.d/10-web.toml
read /etc/emfor/conf.d/99-local.toml
```

### 看合併後的設定

`emfor check-config --show` 把合併後的設定以 TOML 印到標準輸出，每個值後面寫明最後設定它的檔案。`config ok` 那幾行寫到標準錯誤。

```console
# emfor check-config --show 2>/dev/null
# include_dir = "conf.d" (its files are merged below)

[dns]

[services."web.svc.emfor"]
backends = ["10.0.0.21:80", "10.0.0.22:80"] # /etc/emfor/conf.d/10-web.toml
check = "tcp" # /etc/emfor/conf.d/10-web.toml
interval = "5s" # /etc/emfor/conf.d/99-local.toml
```

- 內建預設值不會加進去：你沒寫的 key 不會出現。
- 沒有任何東西被遮蔽。設定裡放的是密碼檔的路徑，不是密碼，但 plugin 的 `command` 會照原樣印出：不要把祕密寫在那裡。
- `emfor check-config --show > merged.toml` 得到的檔案在哪裡都能再通過檢查，因為 `include_dir` 那一行變成了註解。
- 註解的格式是給人看的；不要用程式解析它。
- 無效的設定在標準輸出不印任何東西，並以 2 結束。

## 套用變更：reload

送 `SIGHUP` 給 daemon，讓它重新讀取設定，連同 `include_dir` 裡的檔案。請透過你的服務管理工具來做，它會先檢查設定，設定無效時不送 signal 給 daemon：

```sh
# systemctl reload emfor       # 使用 systemd 的 Linux
# service emfor reload         # FreeBSD
```

reload 是全有或全無。只要設定有任何錯誤（不論在哪個檔案），整份就被拒絕；daemon 記錄每一個錯誤，繼續用舊的設定：

```
ERROR emfor_daemon::cli: invalid config /etc/emfor/emfor.toml: 2 error(s)
ERROR emfor_daemon::cli: defaults.bogus: unknown key
ERROR emfor_daemon::cli: defaults.timeout: timeout must not exceed interval
ERROR emfor_daemon::cli: reload rejected; keeping the previous config
```

reload 能改變什麼：

| 變更 | reload 時 |
| --- | --- |
| 新增或移除服務或 backend | 套用。新的 backend 從 `unknown` 開始。 |
| 服務的 `check` 或 `check_options` | 套用。它的 backend 從 `unknown` 重新開始。 |
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`peers` | 套用；backend 保留原狀態。新的 `interval` 或 `jitter` 也套用到正在進行的等待。 |
| `balance`、`weight`、`site`、`backup`、`drain`、`dns_ttl`、`slow_start` | 套用；backend 保留狀態，正在慢啟動的 backend 保留它的恢復時間。 |
| `[dns] zone`、`ttl`、`rate_limit` | 套用。 |
| `[log] level` | 套用，除非設定了環境變數 `RUST_LOG`。 |
| plugin 的 `command` 或 `env_pass` | 該 plugin 重新啟動，使用它的服務從 `unknown` 重新開始。未變更的 plugin 繼續執行。 |
| `[peers] members`、`secret_file`、`encrypt`、`accept_plaintext` | 套用。新加入的成員從不存活開始。`previous_secret_file` 會檢查，但只在啟動時讀取。 |
| `[store]` 中的任何設定 | 拒絕：`store: store settings require restart`。 |
| `[dns] listen`、新增或移除 `[dns]` | 拒絕：`dns.listen: dns listen settings require restart`。 |
| `[metrics] listen`、新增或移除 `[metrics]` | 拒絕：`metrics.listen: metrics listen settings require restart`。 |
| `[peers] listen`、新增或移除 `[peers]` | 拒絕：`peers.listen: peers listen settings require restart`。 |

被拒絕的這些，要重新啟動 daemon（`systemctl restart emfor`）。重新啟動時有一小段時間，查詢分辨不出好的與壞的 backend（[第 12 章](12-operating.md#重新啟動時應用程式會看到什麼)），所以 reload 夠用時，優先用 reload。

## 產生設定

既然 backend 清單就是設定，總得有東西來寫它：你的設定管理工具（Ansible、Puppet、Salt、樣板），或一個 script。一些好用的做法：

- 在 `conf.d` 裡每個服務（或每個團隊）一個檔案，由工具整個寫出。
- 每行一個 backend，讓 script 可以只改一行（[第 12 章](12-operating.md#用-drain-做滾動部署)的 drain script 就靠這一點）。
- `[peers] members` 單獨放在自己的檔案裡，從雲端 API 寫出（[第 11 章](11-peer-groups.md#從雲端-api-取得成員)）。
- reload 之前，一律以 daemon 的使用者身分執行 `emfor check-config`。
