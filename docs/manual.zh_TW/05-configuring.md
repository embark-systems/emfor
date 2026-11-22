# 5. 設定 emfor

[手冊](README.md) · 上一章：[4. 安裝](04-installing.md) · 下一章：[6. 健康檢查](06-health-checks.md)

這一章談整份設定：格式、怎麼拆成多個檔案、怎麼檢查，以及怎麼套用變更。檢查方式在[第 6 章](06-health-checks.md)與[第 7 章](07-plugins.md)，怎麼選 backend 在[第 8 章](08-choosing-a-backend.md)，每個 key 和它的預設值在[第 18 章](18-config-reference.md)。

## 設定檔的樣子

設定是一個 TOML 檔，預設是 `/etc/emfor/emfor.toml`。只有服務的 `check` 與 `backends` 一定要寫，其他都有預設值。下面是一份貼近實際的小型設定：

```toml
site = "dc1"                       # 這台主機所在的資料中心（選用）

[log]
level = "info"

[dns]                              # 選用的 DNS server，用預設值
[metrics]                          # 選用的 /metrics，用預設值

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
| 最上層 | `site` 與 `include_dir`。它們一定要寫在第一個 `[table]` 之前，因為 TOML 會把 table 標頭後面的 key 都歸到那個 table。 |
| `[store]` | state store 放在哪裡、多大、檔案權限。 |
| `[log]` | log 等級。 |
| `[dns]` | DNS server；有這個 table 才會啟動。 |
| `[metrics]` | Prometheus endpoint；有這個 table 才會啟動。 |
| `[peers]` | 聯防群組（[第 11 章](11-peer-groups.md)）。 |
| `[defaults]` | 服務沒有自己設定時用的值。 |
| `[plugins."<name>"]` | 一個 plugin 程式，服務可以拿它當自己的 `check`。 |
| `[services."<name>"]` | 一個服務：它的檢查、backend 與設定。 |

不管寫在檔案的哪裡，**未知的 key 都算錯誤**，所以拼錯的 key 一定會被抓到。反過來說，較新版本加的 key，舊版本看到就會拒絕整份設定。

## 值的格式

| 種類 | 格式 | 範例 |
| --- | --- | --- |
| 時間長度 | 字串，由一個或多個「數字加單位」組成。單位有 `ms`、`s`、`m`、`h`、`d`，也可以拼出完整的字（`seconds`、`minutes`……）；可以有小數；只寫 `"0"` 表示零。 | `"500ms"`、`"2s"`、`"1.5s"`、`"1h 30m"`、`"2 seconds"` |
| 大小 | 帶 `KiB`、`MiB` 或 `GiB` 的字串，或直接寫位元組數 | `"64MiB"` |
| 檔案權限 | 八進位字串 | `"0640"` |
| 百分比 | 以 `%` 結尾的字串 | `"10%"` |
| 位址 | `IPv4:port` 或 `[IPv6]:port`，port 為 1–65535 | `"10.0.0.11:8080"`、`"[2001:db8::11]:8080"` |

只寫數字不算時間長度：`interval = "2"` 會被拒絕，訊息是 `invalid duration "2", use e.g. "500ms" or "2s"`。

## 設定與預設值

每個服務的時間與選擇相關設定，依序從這些地方取：服務自己的 table、`[defaults]`，最後是內建值：

```toml
[defaults]
interval = "2s"          # 一次檢查結束到下一次開始的時間
timeout = "1s"           # 一次檢查最多能花多久；不能超過 interval
rise = 2                 # 連續通過幾次後變成 healthy
fall = 3                 # 連續失敗幾次後變成 unhealthy
jitter = "10%"           # interval 的隨機浮動，0% 到 50%
on_all_down = "open"     # 或 "closed"
balance = "round-robin"
peers = true             # 有聯防群組就加入
slow_start = "0s"        # 關閉；或 1s 到 1h
```

這正好就是內建的那組值，所以這個 table 什麼都沒改。服務可以覆寫其中任何一項：

```toml
[services."report.svc.emfor"]
check = "tcp"
backends = ["10.0.9.1:5432"]
interval = "30s"
timeout = "5s"
fall = 2
```

## 把設定拆到目錄

設了 `include_dir` 後，daemon 會先讀主設定檔，再讀某個目錄裡的每個 `.toml` 檔，合併成一份設定。這樣主設定檔可以交給套件管，各團隊或工具則各自管自己服務的檔案：

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

**讀哪些檔案、照什麼順序。**

- 檔名以 `.toml` 結尾、不以 `.` 開頭的一般檔案，以及指向一般檔案的連結。其他一律忽略：`web.toml~`、`.web.toml.swp`、`web.toml.disabled`、子目錄。想讓某個檔案退出設定，把它改名成 `.disabled` 結尾就好。
- 照檔名的位元組順序讀，不是照你的地區語言排序。數字要補零：`9-a.toml` 會排在 `10-a.toml` 後面。
- 主設定檔在最底層；後讀的檔案蓋掉先讀的。
- 目錄是空的沒關係，目錄不存在就是錯誤。

**怎麼合併。** 兩個 table 會逐個 key 合併，不管有幾層都一樣。其他東西（數字、字串、清單）一律由後面的值整個取代。所以：

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

`web.svc.emfor` 保留原本的 `check` 與兩個 backend，再加上 `interval = "5s"`。要改 backend，就得把整份清單重寫一次。後面的檔案沒辦法刪掉服務、plugin 或 backend：要從定義它的那個檔案裡拿掉。

**只能放在主設定檔的**：`[store]` 與 `include_dir` 本身。片段裡設了它們會被拒絕（`/etc/emfor/conf.d/60-s.toml: store is only allowed in the main config file`）。其他東西，包括 `site`、`[dns]`、`[peers]` 與 `[plugins]`，放在哪個檔案都可以。

**用原子方式寫檔。** 先用不是 `.toml` 結尾的檔名（或在目錄外面）寫好新檔，再用 `mv` 搬到定位。reload 讀到寫到一半的檔案時會拒絕，舊設定繼續跑；但先寫再改名，就連這次失敗的 reload 都不會發生。

## 誰可以擁有、寫入設定檔

設定可以啟動程式（plugin 的 `command`、`exec` 檢查），這些程式會以 daemon 的使用者身分執行。所以只要設定檔可能被別人改，emfor 就會拒絕：

- **主設定檔**、**`include_dir` 目錄**和**目錄裡的每個檔案**都不能讓 group 或其他人寫入：權限 `0644` 與 `0755` 可以，`0664` 與 `0775` 不行。
- 它們的**擁有者一定要是 root，或 daemon 執行時的使用者**。如果是連結，看的是它指向的那個檔案。

```console
# chmod 0664 /etc/emfor/emfor.toml
# emfor check-config
/etc/emfor/emfor.toml: writable by group or others (mode 0664); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/emfor.toml
```

`emfor server`（啟動時和每次 reload）和 `emfor check-config` 每次讀設定都會做這項檢查。0.7.0 以前只檢查片段，不檢查主設定檔。`emfor status` 與 `emfor lookup` 只讀主設定檔裡的 `[store]` table，不做這項檢查。主設定檔所在的目錄（`/etc/emfor`）不檢查；請像套件那樣，只讓 root 能寫入。

擁有者規則要看是誰在跑指令。請用 daemon 的使用者身分執行 `emfor check-config`，systemd unit 和 FreeBSD 的 `configtest` 都是這樣做。用 root 手動執行時，只接受 root 擁有的檔案。

## 檢查設定

拿本章開頭那份設定來檢查：

```console
# emfor check-config
service db.svc.emfor: check_options has a password_file but tls is not true: the password is not protected on the network, and anyone who can see the traffic, or answer in the backend's place, can get it
config ok: 2 services, 4 backends
```

`emfor check-config` 會讀完整份設定，印出**所有**錯誤。每個錯誤都附上它的 TOML 路徑，設定拆成多個檔案時還會附上檔案。下面的例子是在上面拆開的範例裡多加一個檔案 `conf.d/50-x.toml`，替 `web.svc.emfor` 設了 `timeout = "10s"`：

```console
# emfor check-config
services."web.svc.emfor".timeout: timeout must not exceed interval (in /etc/emfor/conf.d/50-x.toml)
```

有時錯誤牽涉到兩個檔案的值（一個檔案設 `timeout`，另一個設 `interval`）。這時列出的是最後設定錯誤路徑上那個 key 的檔案，這裡就是 `timeout`；即使 `99-local.toml` 比它更晚設定 `interval`，也還是列它。如果沒有任何檔案直接設定那個 key，列出的就是最後設定它上層最近那個 table 的檔案。

它會做這些事：

- 啟動每個宣告過的 plugin，讓 plugin 檢查用到它的服務的 `check_options`，然後停掉它。所以 plugin 選項寫錯，在你 reload 之前就會發現。
- 照檢查時的方式讀密碼檔、CA 檔與 peer 密鑰檔。
- 遇到其他使用者讀得到的密鑰檔、沒走 TLS 就送出的密碼，以及其他使用者能寫入的 store 檔案權限，會發出警告，但不會因此失敗（[第 13 章](13-security.md)）。
- 不會開啟 store，也不需要 daemon 在跑。
- 設定有效時結束代碼是 0，無效時是 2，輸出寫不出去時是 1（磁碟滿了；管線被關掉不算錯誤）。

設了 `include_dir` 時，它接著會照合併順序列出讀過的檔案：

```console
# emfor check-config
config ok: 1 services, 2 backends
read /etc/emfor/emfor.toml
read /etc/emfor/conf.d/10-web.toml
read /etc/emfor/conf.d/99-local.toml
```

### 看合併後的設定

`emfor check-config --show` 會把合併後的設定以 TOML 格式印到標準輸出，每個值後面註明最後是哪個檔案設的。`config ok` 那幾行則寫到標準錯誤。

```console
# emfor check-config --show 2>/dev/null
# include_dir = "conf.d" (its files are merged below)

[dns]

[services."web.svc.emfor"]
backends = ["10.0.0.21:80", "10.0.0.22:80"] # /etc/emfor/conf.d/10-web.toml
check = "tcp" # /etc/emfor/conf.d/10-web.toml
interval = "5s" # /etc/emfor/conf.d/99-local.toml
```

- 不會加上內建預設值：你沒寫的 key 就不會出現。
- 什麼都不遮蔽。設定裡放的是密碼檔的路徑，不是密碼本身，但 plugin 的 `command` 會照原樣印出來：不要把祕密寫在那裡。
- 用 `emfor check-config --show > merged.toml` 存下來的檔案，拿到哪裡都能再通過檢查，因為 `include_dir` 那一行已經變成註解。
- 註解的格式是給人看的，不要寫程式去解析。
- 設定無效時，標準輸出什麼都不印，結束代碼是 2。

## 套用變更：reload

送 `SIGHUP` 給 daemon，它就會重新讀設定，包括 `include_dir` 裡的檔案。請透過你的服務管理工具來做：它會先檢查設定，設定無效就不送 signal 給 daemon：

```sh
# systemctl reload emfor       # 用 systemd 的 Linux
# service emfor reload         # FreeBSD
```

reload 是全有或全無。設定裡只要有任何錯誤，不管在哪個檔案，整份都會被拒絕；daemon 會把每個錯誤記進 log，繼續用舊的設定：

```
ERROR emfor_daemon::cli: invalid config /etc/emfor/emfor.toml: 2 error(s)
ERROR emfor_daemon::cli: defaults.bogus: unknown key
ERROR emfor_daemon::cli: defaults.timeout: timeout must not exceed interval
ERROR emfor_daemon::cli: reload rejected; keeping the previous config
```

reload 能改哪些東西：

| 變更 | reload 時 |
| --- | --- |
| 新增或移除服務、backend | 套用。新的 backend 從 `unknown` 開始。 |
| 服務的 `check` 或 `check_options` | 套用。它的 backend 從 `unknown` 重新開始。 |
| `interval`、`timeout`、`rise`、`fall`、`jitter`、`on_all_down`、`peers` | 套用；backend 保留原本的狀態。新的 `interval` 或 `jitter` 連正在等的那一段也會套用。 |
| `balance`、`weight`、`site`、`backup`、`drain`、`dns_ttl`、`slow_start` | 套用；backend 保留狀態，正在慢啟動的 backend 也保留它的恢復時間。 |
| `[dns] zone`、`ttl`、`rate_limit` | 套用。 |
| `[log] level` | 套用，除非設了環境變數 `RUST_LOG`。 |
| plugin 的 `command` 或 `env_pass` | 該 plugin 會重新啟動，用到它的服務從 `unknown` 重新開始。沒改到的 plugin 繼續跑。 |
| `[peers] members`、`secret_file`、`encrypt`、`accept_plaintext` | 套用。新加入的成員一開始算不存活。`previous_secret_file` 會檢查，但只在啟動時讀取。 |
| `[store]` 裡的任何設定 | 拒絕：`store: store settings require restart`。 |
| `[dns] listen`、新增或移除 `[dns]` | 拒絕：`dns.listen: dns listen settings require restart`。 |
| `[metrics] listen`、新增或移除 `[metrics]` | 拒絕：`metrics.listen: metrics listen settings require restart`。 |
| `[peers] listen`、新增或移除 `[peers]` | 拒絕：`peers.listen: peers listen settings require restart`。 |

被拒絕的這幾項，要重新啟動 daemon 才會生效（`systemctl restart emfor`）。重新啟動時會有一小段時間，查詢分不出哪些 backend 是好的、哪些是壞的（[第 12 章](12-operating.md#重新啟動時應用程式會看到什麼)），所以 reload 能解決的，就優先用 reload。

## 產生設定

既然 backend 清單就是設定，總得有個東西來寫它：你的設定管理工具（Ansible、Puppet、Salt、樣板），或一支 script。幾個好用的做法：

- 在 `conf.d` 裡每個服務（或每個團隊）一個檔案，由工具整個寫出來。
- 每行一個 backend，讓 script 可以只改一行（[第 12 章](12-operating.md#用-drain-做滾動部署)的 drain script 就靠這一點）。
- `[peers] members` 單獨放在一個檔案裡，從雲端 API 產生（[第 11 章](11-peer-groups.md#從雲端-api-取得成員)）。
- reload 之前，一定要先用 daemon 的使用者身分執行 `emfor check-config`。
