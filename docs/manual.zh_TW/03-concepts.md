# 3. 基本概念

[手冊](README.md) · 上一章：[2. 快速上手](02-quick-start.md) · 下一章：[4. 安裝](04-installing.md)

這一章說明手冊其他部分會用到的觀念。每一節最後都會指出哪一章有細節。

## 組成元件

| 元件 | 說明 |
| --- | --- |
| `emfor server` | daemon。讀取設定，檢查每個 backend，並把結果寫入 state store。每台主機一個。 |
| state store | 一個 LMDB 檔案，預設為 `/var/lib/emfor/state.mdb`。daemon 是唯一的寫入者；主機上可以有任意多個行程讀取它。 |
| client 函式庫 | `libemfor`（C，由 Rust crate `emfor-client` 建置），以及 Go、Python、Perl 與 PHP 的包裝。一次查詢讀取 store 並挑出一個 backend。 |
| DNS server | 選用。為各服務回答 A、AAAA 與 SRV 查詢，資料來自 daemon 的記憶體。 |
| `emfor-check` | 一個 plugin 程式，提供資料庫與網路協定的檢查。 |
| `emfor status`、`emfor lookup`、`emfor check-config` | 用來查看 daemon 發布了什麼、一次查詢會回傳什麼，以及設定是否有效的指令。 |
| `/metrics` | 選用的 Prometheus endpoint。 |

沒有中央伺服器。每台主機有自己的 daemon、自己的設定，以及自己看到的 backend 狀況。同一個資料中心的主機可以選擇在[聯防群組](#聯防群組)中分享檢查結果。

## 服務與 backend

**服務**（service）是一個名稱，例如 `db.svc.emfor`，加上：

- 一份 **backend** 清單：提供該服務的 `ip:port` 位址；
- 一個 **check**：如何測試一個 backend（`tcp`、`http`、`exec`，或 `pgsql` 之類的 plugin），以及它的 `check_options`；
- 時間設定：`interval`、`timeout`、`rise`、`fall`、`jitter`；
- 一個 **balance** 方式：如何挑出一個健康的 backend；
- 一個 **on_all_down** 政策：沒有任何 backend 健康時查詢回傳什麼。

```toml
[services."db.svc.emfor"]
check = "tcp"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
```

backend 一律是 IP 位址，從不是主機名稱，這樣 emfor 找 backend 時不依賴 DNS。port 屬於各個 backend；同一個服務的 backend 可以用不同的 port。

服務名稱長得像主機名稱：由點分隔的 label，每個 label 由 `a-z`、`0-9` 與 `-` 組成，最多 253 個字元。不論在設定裡還是查詢時，大小寫與一個結尾的點都會被忽略。

## 檢查結果

每次檢查一個 backend，都以三種**結果**之一結束：

| 結果 | 意義 | 是否算 backend 的失敗 |
| --- | --- | --- |
| `up` | backend 通過。 | 否 |
| `down` | backend 失敗：被拒絕、逾時、回答不對。 | 是 |
| `error` | 因為這台主機上的問題，emfor 無法執行檢查（讀不到密碼檔、plugin 當掉、開啟的檔案太多）。 | 否 |

`down` 或 `error` 帶有一個 **reason**，例如 `timeout` 或 `connect: Connection refused (os error 111)`。你會在 `emfor status`、log 與狀態歷史中看到它。

`down` 與 `error` 的區別很重要：本機的問題不能把每個好的 backend 都標成 down。`error` 不改變任何計數器。只有當一個 backend 只收到 error 的時間超過 `fall × interval`，它才回到 `unknown`。

## 健康狀態、rise 與 fall

每個 backend 有一個**健康狀態**：

| 狀態 | 何時 |
| --- | --- |
| `unknown` | daemon 啟動後、backend 剛加入後、其檢查定義變更後，以及連續很久都是 `error` 結果之後。 |
| `healthy` | 連續 `rise` 次 `up` 結果（預設 2）之後。 |
| `unhealthy` | 連續 `fall` 次 `down` 結果（預設 3）之後。 |

```
               rise × up                     fall × down
   unknown  ─────────────>  healthy  ─────────────────────>  unhealthy
      │                        ^                                 │
      │      fall × down       │            rise × up            │
      └───────────> unhealthy  └─────────────────────────────────┘
```

一次 `up` 會把失敗計數歸零，一次 `down` 會把成功計數歸零，所以計的是連續的結果。計數器讓單一遺失的封包不造成影響。用預設值（`interval = "2s"`、`fall = 3`、`rise = 2`）時，停止回應的 backend 大約 6 秒後被標成 unhealthy，恢復的 backend 大約 4 秒後再次變成 healthy。

backend 處於 `unknown` 期間，emfor 每 200 ms 檢查它一次（`interval` 較短時則每 `interval` 一次），最多 `rise + fall` 次。所以啟動後大約半秒內，每個 backend 的狀態就已確定。細節：[第 6 章](06-health-checks.md#排程)。

## state store 與 heartbeat

daemon 把每個服務寫成 store 裡的一筆記錄，並至少每秒寫入一次 **heartbeat**（目前時間）。client 拿 heartbeat 與自己的時鐘比較：

- heartbeat 最多 3 秒前（`EMFOR_MAX_STALENESS`）：store 是**新鮮**（fresh）的，健康狀態算數；
- 更舊，或為 0（daemon 已正常停止）：store 是**過期**（stale）的，client 把每個 backend 都當作不健康。此時由 `on_all_down` 決定。

因此應用程式絕不會相信已經卡住或消失的 daemon 所留下的健康資料，而 `open` 的服務仍然會給它一個位址。

daemon 啟動時會把每個服務重寫一遍，所有 backend 都是 `unknown`。它從不沿用上一次執行的健康狀態；只有**狀態歷史**（每個 backend 最近 20 次變化）會在重新啟動後保留。細節：[第 12 章](12-operating.md#磁碟上的-store)。

## 查詢回傳什麼：層級

查詢先挑出一組**候選**（candidates），再讓 `balance` 從中選一個。候選是下列**層級**（tier）中第一個不是空的：

1. 最近一次檢查通過的健康主要 backend；
2. 最近一次檢查通過的健康備援 backend；
3. 健康的主要 backend，包含最近一次檢查失敗的；
4. 健康的備援 backend，包含最近一次檢查失敗的。

「最近一次檢查失敗」的意思是 backend 仍是 `healthy`（還沒有失敗到 `fall` 次），但它最近一次檢查沒有通過。因為有第 1 層，開始失敗的 backend 在它**第一次**檢查失敗時就會被排除，只要還有另一個 backend 通過檢查。

四個層級都沒有 backend 時，這個服務就是**全部 down**（all down），由 `on_all_down` 決定：

| `on_all_down` | 查詢 | DNS |
| --- | --- | --- |
| `open`（預設） | 從下列挑選：最近一次檢查通過的 `unknown` backend，否則全部 `unknown` backend，否則全部 backend | 以同樣方式回答 |
| `closed` | 以 `ALL_DOWN` 失敗 | `SERVFAIL` |

當「試一個 backend」比失敗好時（檢查本身可能出錯）用 `open`。當把流量送到壞掉的 backend 比快速失敗更糟時用 `closed`。

`emfor lookup <service> --explain` 會印出層級，以及其他每個 backend 為什麼被排除。細節：[第 8 章](08-choosing-a-backend.md)。

## balance 方式

| `balance` | 選擇 |
| --- | --- |
| `round-robin`（預設） | 輪流選候選 |
| `sticky` | 同一個 affinity key 選同一個 backend |
| `failover` | 依設定檔順序的第一個候選 |
| `weighted` | 依各 backend 的 `weight` 比例 |
| `locality` | 優先選這台主機 `site` 裡的候選 |
| `fastest` | 在檢查回應最快的 backend 之間輪流 |

細節：[第 8 章](08-choosing-a-backend.md#balance-方式)。

## 備援 backend

`backup = true` 的 backend 只在沒有主要 backend 通過檢查時使用（上面的第 2 與第 4 層）。用於另一個資料中心的 replica，或規格較小的待命機器。

## site

設定檔最上層的 `site = "dc1"` 指出這台主機所在的資料中心；backend 上的 `site = "dc2"` 指出該 backend 所在的資料中心。`balance = "locality"` 優先選與主機同一個 site 的 backend，而一個[聯防群組](#聯防群組)只涵蓋一個 site。

## 慢啟動

設定 `slow_start = "30s"` 後，從 `unhealthy` 轉為 `healthy` 的 backend 會在 30 秒內分十個階段逐步增加分到的流量，而不是立刻拿到全部份額，讓冷的快取與連線池有時間暖起來。細節：[第 8 章](08-choosing-a-backend.md#慢啟動)。

## drain

在 backend 上設定 `drain = true`，會把它從所有選擇（查詢與 DNS）中拿掉，同時檢查照跑，用於升級或重新開機。emfor 不在資料路徑上，所以已經建立的連線不受影響；新的查詢只是略過這個 backend。沒有執行期的指令：你修改設定檔然後 reload。細節：[第 8 章](08-choosing-a-backend.md#drain)。

## 聯防群組

沒有協助時，每台主機都需要自己的 `fall` 次檢查失敗，才會注意到故障。在**聯防群組**（peer group）裡，同一個 `site`、在 `[peers]` 中列出彼此的 daemon，會互相交換經過驗證並加密的 UDP 訊息：

- 某個成員看到 backend 從 `healthy` 變成 `unhealthy` 時，通知其他成員，它們會立刻重新檢查那個 backend；
- 存活成員有三台以上時，每個 backend 由其中三台擔任**負責主機**（assigned checker）；三台中有兩台看到它 down 時，每個成員都把它算成 down；
- 不是某個 backend 負責主機的成員，較少檢查它。

查詢仍然只讀本機的 store。連不到其他成員的成員，依自己的檢查決定，和沒有群組時一樣。細節：[第 11 章](11-peer-groups.md)。

## daemon 使用的檔案

| 檔案 | 用途 |
| --- | --- |
| `/etc/emfor/emfor.toml` | 設定檔（可用 `--config` 指定其他路徑）。 |
| `/etc/emfor/conf.d/*.toml` | 設定了 `include_dir = "conf.d"` 時的其他設定檔。 |
| `<store>`（`/var/lib/emfor/state.mdb`） | state store。預設權限 0640。 |
| `<store>-lock` | LMDB 的 lock 檔。每個讀取者都要在裡面登記，所以應用程式需要它的寫入權限。預設權限 0660。 |
| `<store>.writer` | 執行中的 daemon 鎖住它，確保一個 store 只有一個 daemon 寫入。權限 0600。 |
| `<store>.corrupt` | 啟動時被移到一旁的損毀 store。 |
| `<store>-peers` | 有 `[peers]` 時：daemon 用過的 peer 密鑰的指紋（絕不存密鑰本身）。權限 0600。 |
