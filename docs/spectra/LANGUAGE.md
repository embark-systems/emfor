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
| client | client | 各語言（C、Rust、Go、Python、Perl、PHP）的查詢函式庫，對外只提供 `emfor_lookup`，內部讀取本機 LMDB。 | 模組、SDK、driver |

## 狀態與結果

| 標準用語 | 英文 / 識別字 | 定義 | 避免使用 |
| --- | --- | --- | --- |
| 檢查結果 | `up` / `down` / `error` | 單次檢查的結果。`down` 表示後端失敗；`error` 表示檢查器本身出錯（例如外掛當掉），不計入後端失敗。 | 把 `error` 當成 `down` |
| 健康狀態 | 健康 / 不健康 / 未知（healthy / unhealthy / unknown） | 後端經 `rise`/`fall` 累計後的狀態。daemon 重啟後、外掛長時間不可用、或 client 發現心跳過期時為「未知」。 | alive、dead |
| 心跳 | heartbeat | daemon 每秒寫入 LMDB 的全域時間戳。client 發現心跳超過 `max_staleness` 未更新時，視為狀態未知。 | — |
| 全部不健康 | all down（`all_down`、`on_all_down`） | 服務底下沒有任何健康後端的情況。預設 `open`：照樣從所有後端挑選並標記 `all_down`；可設為 `closed`：回傳失敗。 | panic mode |

## 查詢與分配

| 標準用語 | 英文 / 識別字 | 定義 | 避免使用 |
| --- | --- | --- | --- |
| 查詢 | `emfor_lookup(名稱, affinity_key?)` | client 唯一的對外呼叫，傳入服務名稱，回傳一個後端 IP。 | pick、resolve |
| 黏著鍵 | `affinity_key` | 由應用程式從 cookie、header 等處取出的值；同一個 key 在後端健康時永遠分到同一台後端。無狀態：原本的後端恢復健康後，key 會回到它身上。 | **`weight_condition`**（已棄用，容易誤解為權重比例） |
| 分配規則 | rendezvous hashing（HRW） | 有 `affinity_key` 時，對每台候選後端計算 `MD5(key + 後端識別)` 前 4 位元組（big-endian uint32），取最大者。沒有 key 時隨機挑選。為跨語言規格，帶版本號。 | consistent hash ring、ketama |
| 權重 | weight | 依比例挑選後端的設定。**第一版不支援**；不要與 `affinity_key` 混用。 | — |

## 用語修正紀錄

- `weight_condition` → `affinity_key`：原名稱容易被理解為「依權重比例挑選」，實際語意是依 cookie / header 的值做黏著式分配。
