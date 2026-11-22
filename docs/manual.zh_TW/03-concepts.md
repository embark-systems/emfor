# 3. 基本概念

[手冊](README.md) · 上一章：[2. 快速上手](02-quick-start.md) · 下一章：[4. 安裝](04-installing.md)

這一章說明手冊其他章節會用到的觀念。每一節最後都會告訴你細節在哪一章。

## 組成元件

| 元件 | 說明 |
| --- | --- |
| `emfor server` | daemon。它讀設定、檢查每個 backend，再把結果寫進 state store。每台主機跑一個。 |
| state store | 一個 LMDB 檔案，預設是 `/var/lib/emfor/state.mdb`。只有 daemon 會寫它；主機上要有多少行程讀它都可以。 |
| client 函式庫 | `libemfor`（C，從 Rust crate `emfor-client` 建出來），以及 Go、Python、Perl 與 PHP 的包裝。一次查詢會讀 store，挑出一個 backend。 |
| DNS server | 選用。替各個服務回答 A、AAAA 與 SRV 查詢，資料直接來自 daemon 的記憶體。 |
| `emfor-check` | 一個 plugin 程式，內含資料庫與各種網路協定的檢查。 |
| `emfor status`、`emfor lookup`、`emfor check-config` | 用來看 daemon 發布了什麼、一次查詢會回傳什麼，以及設定有沒有效的指令。 |
| `/metrics` | 選用的 Prometheus endpoint。 |

沒有中央伺服器。每台主機有自己的 daemon、自己的設定，看到的 backend 狀況也是自己的。同一個資料中心的主機可以選擇加入[聯防群組](#聯防群組)，互相分享檢查結果。

## 服務與 backend

**服務**（service）是一個名稱，例如 `db.svc.emfor`，再加上：

- 一份 **backend** 清單：提供這個服務的 `ip:port` 位址；
- 一個 **check**：怎麼測試一個 backend（`tcp`、`http`、`exec`，或 `pgsql` 之類的 plugin），以及它的 `check_options`；
- 時間設定：`interval`、`timeout`、`rise`、`fall`、`jitter`；
- 一個 **balance** 方式：怎麼挑出一個健康的 backend；
- 一個 **on_all_down** 政策：沒有任何 backend 健康時，查詢要回傳什麼。

```toml
[services."db.svc.emfor"]
check = "tcp"
balance = "failover"
backends = ["10.0.3.1:5432", "10.0.3.2:5432"]
```

backend 一律寫 IP 位址，不寫主機名稱，這樣 emfor 找 backend 時就不必靠 DNS。port 是跟著每個 backend 走的；同一個服務的 backend 可以用不同的 port。

服務名稱的寫法跟主機名稱一樣：用點分隔的多個 label，每個 label 由 `a-z`、`0-9` 與 `-` 組成，總長最多 253 個字元。不論在設定裡還是查詢時，大小寫都不分，結尾的一個點也會忽略。

## 檢查結果

每次檢查一個 backend，結束時都會得到三種**結果**之一：

| 結果 | 意義 | 算不算 backend 的失敗 |
| --- | --- | --- |
| `up` | backend 通過。 | 不算 |
| `down` | backend 失敗：連線被拒、逾時、回答不對。 | 算 |
| `error` | 這台主機自己出了問題，emfor 沒辦法做檢查（密碼檔讀不到、plugin 當掉、開啟的檔案太多）。 | 不算 |

`down` 或 `error` 會帶一個 **reason**，例如 `timeout` 或 `connect: Connection refused (os error 111)`。`emfor status`、log 和狀態歷史裡都看得到它。

分清楚 `down` 和 `error` 很重要：本機的問題不能讓所有正常的 backend 都被標成 down。`error` 不會動到任何計數器。一個 backend 只拿到 error、而且持續超過 `fall × interval`，才會回到 `unknown`。

## 健康狀態、rise 與 fall

每個 backend 都有一個**健康狀態**：

| 狀態 | 何時 |
| --- | --- |
| `unknown` | daemon 剛啟動、backend 剛加入、backend 的檢查定義改了之後，以及連續很長一段時間都是 `error` 結果之後。 |
| `healthy` | 連續 `rise` 次 `up` 結果（預設 2）之後。 |
| `unhealthy` | 連續 `fall` 次 `down` 結果（預設 3）之後。 |

```
               rise × up                     fall × down
   unknown  ─────────────>  healthy  ─────────────────────>  unhealthy
      │                        ^                                 │
      │      fall × down       │            rise × up            │
      └───────────> unhealthy  └─────────────────────────────────┘
```

一次 `up` 會把失敗次數歸零，一次 `down` 會把成功次數歸零，所以算的是連續的結果。有了計數器，單一個封包遺失也沒有影響。用預設值（`interval = "2s"`、`fall = 3`、`rise = 2`）時，backend 停掉後大約 6 秒會被標成 unhealthy，恢復後大約 4 秒會變回 healthy。

backend 還是 `unknown` 的時候，emfor 每 200 ms 檢查一次（如果 `interval` 更短，就每 `interval` 一次），最多檢查 `rise + fall` 次。所以啟動後大約半秒內，每個 backend 的狀態就確定了。細節見[第 6 章](06-health-checks.md#排程)。

## state store 與 heartbeat

daemon 會把每個服務寫成 store 裡的一筆記錄，並且至少每秒寫一次 **heartbeat**（目前時間）。client 會拿 heartbeat 和自己的時鐘比較：

- heartbeat 距今最多 3 秒（`EMFOR_MAX_STALENESS`）：store 是**新鮮**（fresh）的，健康狀態算數；
- 更舊，或是 0（daemon 已經正常停止）：store 是**過期**（stale）的，client 會把每個 backend 都當成不健康，接下來由 `on_all_down` 決定。

所以，daemon 卡住或不見了，應用程式也絕不會相信它留下的健康資料；而 `open` 的服務還是會給應用程式一個位址。

daemon 啟動時會把每個服務重寫一遍，所有 backend 都設成 `unknown`。它從不沿用上一次執行的健康狀態；重新啟動後只有**狀態歷史**（每個 backend 最近 20 次變化）會留下來。細節見[第 12 章](12-operating.md#磁碟上的-store)。

## 查詢回傳什麼：層級

查詢會先挑出一組**候選**（candidates），再交給 `balance` 從裡面選一個。候選取自下列**層級**（tier）中第一個不是空的：

1. 最近一次檢查通過的健康主要 backend；
2. 最近一次檢查通過的健康備援 backend；
3. 健康的主要 backend，包括最近一次檢查失敗的；
4. 健康的備援 backend，包括最近一次檢查失敗的。

「最近一次檢查失敗」是指 backend 還是 `healthy`（失敗次數還沒到 `fall`），但最近那一次檢查沒過。因為有第 1 層，只要還有別的 backend 通過檢查，開始出問題的 backend 在**第一次**檢查失敗時就會被排除。

四個層級都沒有 backend 時，這個服務就是**全部 down**（all down），由 `on_all_down` 決定怎麼做：

| `on_all_down` | 查詢 | DNS |
| --- | --- | --- |
| `open`（預設） | 依序從這些裡挑：最近一次檢查通過的 `unknown` backend；沒有的話，全部 `unknown` backend；再沒有的話，全部 backend | 用同樣的方式回答 |
| `closed` | 失敗，回傳 `ALL_DOWN` | `SERVFAIL` |

如果「試一個 backend」比直接失敗好（檢查本身可能出錯），就用 `open`。如果把流量送到壞掉的 backend 比快速失敗更糟，就用 `closed`。

`emfor lookup <service> --explain` 會印出用的是哪一層，以及其他每個 backend 為什麼被排除。細節見[第 8 章](08-choosing-a-backend.md)。

## balance 方式

| `balance` | 選擇 |
| --- | --- |
| `round-robin`（預設） | 候選輪流 |
| `sticky` | 同一個 affinity key 選同一個 backend |
| `failover` | 依設定檔順序的第一個候選 |
| `weighted` | 依各 backend 的 `weight` 比例 |
| `locality` | 先選這台主機所在 `site` 的候選 |
| `fastest` | 在檢查回應最快的那些 backend 之間輪流 |

細節見[第 8 章](08-choosing-a-backend.md#balance-方式)。

## 備援 backend

設了 `backup = true` 的 backend，只有在沒有任何主要 backend 通過檢查時才會用到（上面的第 2 與第 4 層）。適合用在另一個資料中心的 replica，或規格較小的待命機器。

## site

設定檔最上層的 `site = "dc1"` 表示這台主機在哪個資料中心；寫在 backend 上的 `site = "dc2"` 表示那個 backend 在哪個資料中心。`balance = "locality"` 會優先選和主機同一個 site 的 backend，而一個[聯防群組](#聯防群組)只涵蓋一個 site。

## 慢啟動

設了 `slow_start = "30s"` 後，從 `unhealthy` 變回 `healthy` 的 backend 不會馬上拿到全部的流量份額，而是在 30 秒內分十個階段慢慢增加，讓冷的快取和連線池有時間暖起來。細節見[第 8 章](08-choosing-a-backend.md#慢啟動)。

## drain

在 backend 上設 `drain = true`，就會把它從所有選擇（查詢與 DNS）中拿掉，但檢查照跑，適合升級或重新開機時用。emfor 不在資料路徑上，所以已經建立的連線不受影響，只是新的查詢會跳過這個 backend。沒有執行期的指令可用：要改設定檔再 reload。細節見[第 8 章](08-choosing-a-backend.md#drain)。

## 聯防群組

沒有其他主機幫忙時，每台主機都要自己檢查失敗 `fall` 次，才會發現故障。在**聯防群組**（peer group）裡，同一個 `site`、在 `[peers]` 裡列出彼此的 daemon，會互相交換經過驗證、加密的 UDP 訊息：

- 某個成員看到 backend 從 `healthy` 變成 `unhealthy`，就通知其他成員，大家會立刻重新檢查那個 backend；
- 存活的成員有三台以上時，每個 backend 會由其中三台當**負責主機**（assigned checker）；三台裡有兩台看到它 down，所有成員都把它算成 down；
- 不是某個 backend 負責主機的成員，檢查它的頻率會比較低。

查詢還是只讀本機的 store。連不到其他成員的成員，就照自己的檢查結果判斷，跟沒有群組時一樣。細節見[第 11 章](11-peer-groups.md)。

## daemon 使用的檔案

| 檔案 | 用途 |
| --- | --- |
| `/etc/emfor/emfor.toml` | 設定檔（要用別的路徑，就加 `--config`）。 |
| `/etc/emfor/conf.d/*.toml` | 設了 `include_dir = "conf.d"` 時讀的其他設定檔。 |
| `<store>`（`/var/lib/emfor/state.mdb`） | state store。預設權限 0640。 |
| `<store>-lock` | LMDB 的 lock 檔。每個讀取者都要在裡面登記，所以應用程式要有它的寫入權限。預設權限 0660。 |
| `<store>.writer` | 執行中的 daemon 會鎖住它，確保一個 store 只有一個 daemon 在寫。權限 0600。 |
| `<store>.corrupt` | 啟動時發現損毀、被移到一旁的 store。 |
| `<store>-peers` | 有 `[peers]` 時才有：daemon 用過的 peer 密鑰的指紋（絕不存密鑰本身）。權限 0600。 |
