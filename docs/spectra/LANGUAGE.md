# emfor 用語表

本檔記錄 emfor 專案的標準用語。規格、提案、設計文件與程式碼註解應使用「標準用語」欄的詞；「避免使用」欄列出容易混淆的說法。

## 核心概念

| 標準用語 | 英文 / 識別字 | 定義 | 避免使用 |
| --- | --- | --- | --- |
| emfor daemon | `emfor` | 每台應用主機各跑一份的 Rust 常駐程式，負責健康檢查、寫入 LMDB、提供本機 DNS。實際流量不經過它。 | proxy、負載平衡器（emfor 不轉送流量） |
| 服務 | service | 一組提供相同功能的後端，以 hostname 命名（例如 `api.svc.emfor`）。DNS 查詢與 client 查詢使用同一個名稱。 | pool、cluster、upstream |
| 後端 | backend | 服務底下的一台實際機器，以設定檔中的原始字串識別（例如 `"10.0.0.11:8080"`）。 | server、node、instance |
| 檢查器 | checker | 執行單次健康檢查的實作，透過統一介面（`validate`、`check`）被核心呼叫。tcp、http、exec 為內建檢查器。 | probe、monitor |
| 外掛 | plugin | 以常駐子程序形式運作的外部檢查器，透過 stdin/stdout 逐行 JSON 與 emfor 溝通，須在設定檔 `[plugins.*]` 註冊。 | extension、module（「模組」保留給 client） |
| 負責網域 | `zone`（`[dns] zone`） | emfor DNS 負責回答的網域，預設 `svc.emfor`。只有名稱在這個網域底下的服務能用 DNS 查到；網域以外的查詢回 `REFUSED`。 | domain、suffix |
| 本機 resolver | resolver | 應用主機上負責所有 DNS 查詢的程式（systemd-resolved、dnsmasq、unbound、FreeBSD `local_unbound`），依網域把負責網域的查詢轉發給 emfor。emfor 本身不是 resolver。 | DNS server（指 emfor 時） |
| client | client | 各語言（C、Rust、Go、Python、Perl、PHP）的查詢函式庫，對外只提供一個查詢呼叫，內部讀取本機 LMDB：C `emfor_lookup`、Rust `emfor_client::lookup`、Go `emfor.Lookup`、Python `emfor.lookup`、Perl `Emfor::lookup`、PHP `Emfor\lookup`。Rust 直接用 crate；其他語言都包裝 `libemfor`（Go 用 cgo、Python 用 ctypes、Perl 用 XS、PHP 用 C extension）。 | 模組、SDK、driver |
| libemfor | `libemfor`（`.so` / `.dylib`） | 由 Rust crate `emfor-client` 經 cargo-c 產出的 C 函式庫，內嵌與 daemon 同一份 LMDB，C、Go、Python、Perl、PHP client 都包裝它的動態函式庫。必須與 emfor 同版本一起升級。 | 系統 liblmdb、PHP dba（lock 格式與 daemon 不同，讀不了 store） |

## 狀態與結果

| 標準用語 | 英文 / 識別字 | 定義 | 避免使用 |
| --- | --- | --- | --- |
| 檢查結果 | `up` / `down` / `error` | 單次檢查的結果。`down` 表示後端失敗；`error` 表示檢查器本身出錯（例如外掛當掉），不計入後端失敗。 | 把 `error` 當成 `down` |
| 健康狀態 | 健康 / 不健康 / 未知（healthy / unhealthy / unknown） | 後端經 `rise`/`fall` 累計後的狀態。daemon 重啟後、外掛長時間不可用、或 client 發現心跳過期時為「未知」。 | alive、dead |
| 心跳 | heartbeat | daemon 每秒寫入 LMDB 的全域時間戳。client 發現心跳超過 `max_staleness`（預設 3 秒）未更新、或 daemon 關閉時歸零，視為狀態未知：所有後端當成不健康，依 `on_all_down` 處理（`open` 照樣挑選，`closed` 回 `ALL_DOWN`）。 | — |
| 全部不健康 | all down（`all_down`、`on_all_down`） | 服務底下沒有任何健康後端的情況。預設 `open`：照樣從所有後端挑選並標記 `all_down`；可設為 `closed`：回傳失敗。 | panic mode |

## 查詢與分配

| 標準用語 | 英文 / 識別字 | 定義 | 避免使用 |
| --- | --- | --- | --- |
| 查詢 | `emfor_lookup(名稱, affinity_key?)` | client 唯一的對外呼叫，傳入服務名稱，回傳一個後端的 `ip:port` 字串（即 LMDB 後端記錄的 `id`，IPv6 為 `[ip]:port`）。設定只透過環境變數 `EMFOR_STORE`、`EMFOR_MAX_STALENESS` 覆寫預設值。 | pick、resolve |
| 錯誤碼 | `UNAVAILABLE` / `INCOMPATIBLE` / `NOT_FOUND` / `ALL_DOWN` | 查詢失敗的原因，各語言共用：LMDB 打不開或 lock 格式不符（`libemfor` 與 daemon 版本不同）／格式版本或記錄無法解析／查無服務／沒有健康後端且 `on_all_down = "closed"`。各語言依自身慣例回報（回傳碼、`Result`、`error`、exception）。 | `STALE`（心跳過期不是獨立錯誤） |
| 黏著鍵 | `affinity_key` | 由應用程式從 cookie、header 等處取出的值；同一個 key 在後端健康時永遠分到同一台後端。無狀態：原本的後端恢復健康後，key 會回到它身上。 | **`weight_condition`**（已棄用，容易誤解為權重比例） |
| 分配方式 | `balance` | 每個服務在設定檔指定的挑選後端方式，daemon 寫進 LMDB 服務記錄，client 與 DNS 都依它挑選。值見下表「分配方式的值」。 | 分配規則、策略、algorithm |
| 黏著分配演算法 | rendezvous hashing（HRW） | `sticky` 使用：對每台候選後端計算 `MD5(key + 後端識別)` 前 4 位元組（big-endian uint32），取最大者。為跨語言規格，帶版本號。 | consistent hash ring、ketama |
| 權重 | weight | 依比例挑選後端的設定，供 `weighted` 使用。**第一版不支援**；不要與 `affinity_key` 混用。 | — |

### 分配方式的值

| 值 | 行為 | 狀態 |
| --- | --- | --- |
| `round-robin` | 在健康後端間輪流。DNS 由 daemon 做全域輪流；client 只能在單一程序內輪流（起點隨機），因為 client 唯讀 LMDB、程序間沒有共用計數器。忽略 `affinity_key`。**預設值。** | 第一版 |
| `sticky` | 有 `affinity_key` 時用 rendezvous hashing 固定分到同一台；沒有 key 時（含 DNS）改用 `round-robin`。 | 第一版 |
| `failover` | 永遠回設定檔順序中第一台健康的後端，掛了才換下一台。忽略 `affinity_key`。 | 第一版 |
| `random` | 從健康後端中均勻隨機挑選。 | 規劃中（client 端與 `round-robin` 差異小） |
| `fastest` | 挑最近一次檢查延遲最低的後端；LMDB 需記錄檢查延遲，並設計避免所有 client 同時湧向同一台的分散機制。 | 規劃中 |
| `weighted` | 依後端 `weight` 比例挑選。 | 規劃中 |
| `zone` | 優先挑與 client 同機房（標籤相符）的後端，沒有才挑其他。後端需能加標籤，client 需知道自己所在機房。 | 規劃中 |

全部後端不健康且 `on_all_down = "open"` 時，以同一種分配方式從所有後端中挑選。不存在 least-connections：流量不經過 emfor，無從得知連線數。

## 用語修正紀錄

- `weight_condition` → `affinity_key`：原名稱容易被理解為「依權重比例挑選」，實際語意是依 cookie / header 的值做黏著式分配。
- 查詢回傳值「後端 IP」→ `ip:port`：同一服務的後端 port 可以不同，只回 IP 會讓應用程式連錯 port；`ip:port` 與設定檔、LMDB `id` 相同。經 DNS 仍只能得到 IP（SRV 屬後續規劃）。
- 「分配規則」→ 分配方式（`balance`）：原本只有黏著與隨機兩種固定行為，現在改為每個服務在設定檔選擇的方式；rendezvous hashing 改稱黏著分配演算法，只屬於 `sticky`。
- client 讀 LMDB 的方式「各語言用自己的 LMDB 套件」→ `libemfor`：相容性驗證發現系統 liblmdb 與 PHP dba 的 lock 格式和 daemon 不同，讀不了 store。
