# 1. 基本概念

[手冊](README.md) · 下一章：[2. 安裝](02-installation.md)

## emfor 做什麼

emfor 是在每一台應用程式主機上執行的健康檢查 daemon。它檢查應用程式所用服務的 backend，並把哪些 backend 健康寫進同一台主機上的一個檔案。應用程式需要（例如）一台資料庫伺服器時，向 emfor client 函式庫要一個；函式庫讀取本機檔案，回傳像 `10.0.3.1:5432` 這樣的位址。查詢不經過網路、大約一微秒完成，而且在其他主機或網路故障時仍然有效。

```
              checks over the network
  backends <------------------------------ emfor daemon ---------> DNS on 127.0.0.1
                                               |  writes           (programs without a
                                               v                    client; answered from
                                     local LMDB state store         the daemon's memory)
                                               |  reads
                             +-----------------+-----------+
                             |                             |
                  client library (Rust, C,           emfor status
                  Go, Python, Perl, PHP)
```

每台主機執行自己的 daemon、用自己的設定，從自己的位置看 backend。沒有中央伺服器。同一個機房的 daemon 可以選擇組成[聯防群組](#聯防群組)，互相通知故障、對每個 backend 投票；查詢仍然只讀本機的 store。

## 組成元件

| 元件 | 說明 |
| --- | --- |
| `emfor server` | daemon。讀取 `emfor.toml`，反覆檢查每個 backend，並把結果寫入 state store。每台主機一個（每個 store 一個）。 |
| state store | 一個 LMDB 檔案，預設為 `/var/lib/emfor/state.mdb`，每個服務一筆記錄。daemon 是唯一的寫入者；可以有任意多個行程讀取。 |
| client 函式庫 | `emfor_client`（Rust）與 `libemfor`（C），以及 Go、Python、Perl、PHP 的包裝。一次查詢讀取 store 並選出一個 backend。 |
| DNS server | 選用。為某個 zone 內的服務名稱回答 A、AAAA 與 SRV 查詢，資料來自 daemon 的記憶體，給無法使用 client 函式庫的程式用。 |
| `emfor status` | 顯示 daemon 發布的內容，也可以當作監控檢查。 |
| `/metrics` | 選用的 Prometheus endpoint。 |
| `emfor-check` | 一個 plugin 程式，提供 redis、MySQL/MariaDB、PostgreSQL 與 LDAP 的檢查。 |

## 服務與 backend

**服務**（service）是一個名稱，例如 `api.svc.emfor`，加上一份 **backend** 清單：提供該服務的 `ip:port` 位址。每個服務有一個 **check**（如何測試一個 backend）、時間設定、一個 **balance** 方式（如何在健康的 backend 中挑選），以及一個 **on_all_down** 政策（沒有任何 backend 健康時怎麼辦）。

```toml
[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
```

backend 一律是 IP 位址，不是主機名稱：emfor 找 backend 不依賴 DNS。

## 檢查結果與健康狀態

每次檢查都以一個**結果**結束：

| 結果 | 意義 | 是否算 backend 的失敗 |
| --- | --- | --- |
| `up` | backend 通過。 | 否 |
| `down` | backend 失敗（被拒絕、逾時、回答不對）。 | 是 |
| `error` | emfor 無法執行檢查（本機問題，例如讀不到密碼檔或 plugin 當掉）。 | 否 |

每個 backend 有一個**健康狀態**：

| 狀態 | 何時 |
| --- | --- |
| `unknown` | daemon 啟動後、backend 剛加入後、其 `check` 變更後，以及連續很久都是 `error` 之後。 |
| `healthy` | 連續 `rise` 次 `up`（預設 2）之後。 |
| `unhealthy` | 連續 `fall` 次 `down`（預設 3）之後。 |

計數器讓單一遺失的封包不造成影響：一次失敗不會移除 backend，一次成功也不會讓它回來。`error` 不改變任何東西，所以檢查端主機的問題不會把好的 backend 標成 down；只有當一個 backend 只收到 error 的時間超過 `fall × interval`，它才回到 `unknown`。

## 查詢回傳什麼

client 以服務中**健康**的 backend 作為候選，交給 `balance` 挑一個。沒有任何 backend 健康時，由 `on_all_down` 決定：

- `open`（預設）：應用程式照樣拿到一個 backend；當出錯的是檢查本身而不是 backend 時，這通常比直接失敗好。仍是 `unknown`（還沒有定論，或檢查只回錯誤）的 backend 優先於已確定 `unhealthy` 的；見[第 5 章](05-backend-selection.md#沒有任何-backend-健康時)。
- `closed`：查詢以 `ALL_DOWN` 失敗，DNS 回答 `SERVFAIL`。

## Heartbeat 與過期的 store

daemon 至少每秒把一次 **heartbeat**（目前時間）寫進 store。client 拿它與自己的時鐘比較：heartbeat 超過 3 秒（`EMFOR_MAX_STALENESS`），或 daemon 已停止並把它設為 0 時，store 就是**過期**（stale）的。在過期的 store 裡，client 把每個 backend 都當作不健康，由 `on_all_down` 決定結果。因此應用程式絕不會相信已經卡住或消失的 daemon 所留下的健康資料，而 `open` 的服務仍然能拿到位址。

daemon 啟動時會把每筆服務記錄重寫一遍，所有 backend 都是 `unknown`；它從不沿用上一次執行的健康狀態。backend 處於 `unknown` 期間大約每 200 ms 檢查一次，所以用預設值時，啟動後約半秒內每個 backend 的狀態就已確定。

## 聯防群組

同一個機房的主機看到的是同一批 backend。沒有協助時，每個 daemon 都要靠自己的 `fall` 次檢查失敗，才會發現某個 backend 掛了。在**聯防群組**（peer group）裡，同一個 `site`、在 `[peers]`（[第 3 章](03-configuration.md#peers)）中列出彼此的 daemon，會互相傳送經過驗證的 UDP 訊息：

- 每秒一次 heartbeat，讓每個成員知道其他成員是否存活；
- 某個成員看到 backend 從 `healthy` 變成 `unhealthy` 時，通知其他所有成員；它們會立刻重新檢查那個 backend，之後每 200 ms 一次、最多 `fall` 次，而不是等到下一個 `interval`。

通知本身從不改變 backend 的狀態，改變狀態的是**多數決**：存活成員（含本機）至少三台時，每個 backend 有三台**負責主機**（assigned checker），由服務、backend 與每個成員位址的雜湊從存活成員中挑出，所以每個成員挑到的都是同樣三台。負責主機每次自己檢查後，把結果（`up` 或 `down`）送給其他成員。每個成員每次自己檢查後，計算該 backend 負責主機送來、夠新（在送出者最近三個 interval 之內）、而且檢查定義相同的結果：

- 兩票以上 `down`：這台主機也把 backend 算成 down，即使本機的檢查通過；原因寫成 `peers 2/3: <原因>`（down 票數／計入的票數）；
- 否則以本機的結果為準，`down` 的原因寫成 `local: <原因>`：這台主機看到其他主機沒看到的問題；
- 存活成員少於三台，或一票都沒有時，本機結果原樣使用，和沒有群組時一樣。

算出的結果再照常經過 `rise` 與 `fall`。群組判為 down、而本機檢查通過的 backend，`emfor status` 會標上 `local-up`。查詢仍然只讀本機的 store。連不到的成員不會造成任何改變：投票退回各 daemon 自己的檢查。沒有 master，群組也不會跨兩個機房。某個服務可以用 `peers = false` 不參加群組（[第 3 章](03-configuration.md#peers)）；這台主機會告訴其他成員，它們再從其餘成員中挑這個服務的負責主機。

檢查也會分工。不是某個 backend 負責主機的成員，在兩台負責主機意見一致時不檢查它：兩票 `up` 算 `up`，兩票 `down` 算 `down`（`peers 2/3:`）。它仍然至少每三個 `interval` 用服務原本的 `check` 自己檢查一次，而一致的票少於兩張時每次都自己檢查。負責主機認為 backend 正常、本機檢查卻失敗時，它會立刻快速重查（每 200ms 一次，最多 `fall` 次，和收到其他成員通知時一樣），之後每個 `interval` 都檢查，直到通過為止，所以只有這台主機看到的故障仍然寫成 `local: <原因>`；這種故障要大約三個 `interval` 加上 `fall` 次快速檢查才會發現，而不是 `fall` 次檢查。有三台負責主機、N 個成員時，每個 backend 每個 `interval` 大約被檢查 3 + (N − 3) / 3 次，而不是 N 次。

## 故障行為一覽

| 什麼故障 | 應用程式看到什麼 |
| --- | --- |
| 一個 backend | 經過 `fall` 次失敗的檢查後（預設約 6 秒）變成 `unhealthy`，不再被回傳。在聯防群組裡，只要一個成員發現，其他成員約一秒內跟上；三台負責主機中有兩台看到它 down 時，每個成員在 `fall` 次檢查後都把它標成 down，即使它在那台主機上仍有回應。只有一台非負責主機看到的故障，在那台主機上最多會晚大約三個 `interval` 才發現，但之後只要 `fall` 次間隔 200ms 的檢查，而不是 `fall` 個 `interval`（預設值下最慢約 6.4 秒在那台主機上標成 down，沒有聯防群組時約 6 秒）。 |
| 某服務的所有 backend | `open`：有 `unknown` 的 backend 時用它，否則任一個 backend；`closed`：`ALL_DOWN` / `SERVFAIL`。 |
| emfor daemon 停止或卡住 | 3 秒後 store 過期：`open` 的服務回傳任一 backend，`closed` 的服務失敗。查詢永遠不會卡住。 |
| store 檔案不存在或無法讀取 | 查詢以 `UNAVAILABLE` 失敗。 |
| 檢查端主機斷網 | 檢查失敗，backend 變成 `unhealthy`；這台主機上的應用程式本來也連不到它們。 |
| plugin 當掉 | 它的檢查都是 `error`（狀態不變），直到 daemon 把它重新啟動。 |

## daemon 使用的檔案

| 檔案 | 用途 |
| --- | --- |
| `/etc/emfor/emfor.toml` | 設定檔。可用 `--config` 指定其他路徑。 |
| `<store>`（`/var/lib/emfor/state.mdb`） | state store。 |
| `<store>-lock` | LMDB 的 lock 檔。每個讀取者都要在裡面登記，所以應用程式需要它的寫入權限。 |
| `<store>.writer` | 執行中的 daemon 鎖住它，確保一個 store 只有一個 daemon 寫入。 |
| `<store>.corrupt` | 啟動時被移到一旁的損毀 store，留著供檢查。 |
