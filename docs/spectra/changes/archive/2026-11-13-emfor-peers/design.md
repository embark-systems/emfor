## Context

每台應用主機跑一份 emfor daemon，各自讀自己的設定、檢查所有後端、寫本機 LMDB store；docs/manual/01-concepts.md 寫明「沒有中央伺服器，emfor daemon 之間也不溝通」。檢查排程在 `schedule.rs` 的 `spawn_backend`：每個後端一個 tokio task，等待 → 檢查 → `BackendHealth::apply`（rise / fall）→ 送 `WriterMsg::Update` 給 writer。`supervisor.rs` 的 `Supervisor::sync` 在啟動與重載時建立、保留或停止這些 task。最上層 `site` 已存在（`Config::site`，`locality` 使用），sticky 已用 rendezvous hashing。

同一個機房裡多台主機看到的是同一批後端。後端壞掉時，每台主機各自要 `fall × interval`（預設 3 × 2s）才判定，而且每台都各自對後端送一份檢查。使用者在討論中決定加入「聯防群組」（LANGUAGE.md）：同一個 `site`、列在彼此 `[peers]` 的 daemon 互相交換檢查結果；交換的資料只是本機判斷的參考，查詢仍只讀本機 store；不跨 `site`、沒有 master。

完整目標分三個 change 完成，本 change 是第一階段（見下方「三階段完整目標」）。docs/spectra/config.yaml 的專案說明寫著「第一版不做：多台 emfor 同步」，本 change 一併改寫。

## Goals / Non-Goals

**Goals:**

- 建立聯防群組的基礎：`[peers]` 設定、UDP 傳輸、HMAC-SHA256 認證與防重送、成員存活、每個服務的檢查定義摘要。
- 故障快速通知：一台主機看到後端由 `healthy` 變 `unhealthy`，其他成員立刻重查並以快節奏連查，整個機房更快把它標為 `unhealthy`。
- 每個後端 task 有「立刻重查」通道，後兩個 change 直接沿用。
- 沒有 `[peers]` 時行為與現在完全相同；成員全部失聯時每台主機照常獨立檢查。
- 文件寫清楚三階段完整目標，讓 `emfor-peers-quorum` 與 `emfor-peers-split-checks` 照著做。

**Non-Goals:**

- 多數決與「全區 down 優先」（`emfor-peers-quorum`）。本階段收到的通知不會直接改變任何後端的狀態。
- 分工檢查、本機 TCP 探測、負責主機重算（`emfor-peers-split-checks`）。本階段每台主機仍檢查所有後端。
- `emfor status` 顯示成員或全區與本機不一致（`emfor-peers-quorum`）。LMDB 格式、client、DNS 都不變。
- 自動發現成員（multicast、gossip）、跨 `site` 聯防、master / 選主、加密訊息內容（只認證、不加密：內容只有服務名稱、後端位址與原因，同一機房內可見）。
- TCP 傳輸或 TLS：通知遺失只是失去加速，不影響正確性，不值得連線管理的複雜度。

## Decisions

### `[peers]` 設定：同一份成員清單、listen 必須是其中之一

```toml
site = "dc1"                      # 有 [peers] 時必填

[peers]
listen = "10.0.0.5:5302"          # 本機 UDP 位址，必須出現在 members
members = ["10.0.0.5:5302", "10.0.0.6:5302", "10.0.0.7:5302"]
secret_file = "/etc/emfor/peers.key"
```

`members` 列出群組所有成員、包含本機，所以同一個 `site` 的每台主機可以放同一份清單，只有 `listen` 不同。三個鍵都必填。`listen` 不可為 `0.0.0.0` 或 `::`：它同時是本機在群組裡的身分，收到的訊息也以來源位址比對 `from`。預設埠 5302 只出現在文件與範例中（5300 是 DNS、5301 是 metrics）。成員數 2 到 64；上限是估計值（每秒心跳 64 × 63 則、每則約 200 bytes，約 0.8 MB/s 全群組總量），尚未實測，超過時再另議。

替代方案：自動發現（multicast）—— 機房網路常擋 multicast，且新主機自動加入等於把信任交給網路；`members` 不含本機 —— 每台主機的清單都不同，容易抄錯。

### UDP 訊息格式：魔數、HMAC 標籤、JSON 內容

一則訊息是一個 UDP datagram：

| 位元組 | 內容 |
| --- | --- |
| 0–3 | `EMP1`（魔數，含格式版本 1） |
| 4–35 | HMAC-SHA256（密鑰為 `secret_file` 內容），計算範圍為魔數加上 JSON 內容 |
| 36– | UTF-8 JSON 物件 |

JSON 共同欄位：`from`（傳送者的 `listen`，`ip:port` 字串）、`site`、`boot`（傳送者啟動時隨機產生的 64 位元值，16 個小寫 hex）、`seq`（從 1 起每則加 1）、`ts`（傳送時的 Unix 毫秒）、`kind`。

- `kind = "heartbeat"`：沒有其他欄位。
- `kind = "down"`：另有 `service`、`digest`、`backend`（後端 id）、`reason`（UTF-8 不超過 256 bytes，超過時在字元邊界截斷）。

一則 datagram 最多 1200 bytes（避免分片），超過的收到即丟。JSON 中不認得的欄位忽略；通過驗證但 `kind` 不認得的訊息只用來更新存活，不計為被拒——後兩個 change 新增訊息種類時，舊版主機在滾動升級期間仍把新版主機視為存活。

JSON 而非自訂二進位：除錯時 `tcpdump -A` 可直接讀；每秒每成員一則，編解碼成本不重要。

### 訊息認證與防重送

收到的 datagram 依序檢查，第一個失敗的項目決定被拒原因：

1. 長度至少 36 bytes、不超過 1200、魔數為 `EMP1`、JSON 可解析且共同欄位齊全 —— 否則 `malformed`。
2. HMAC 標籤以目前密鑰或（換密鑰後 120 秒內的）前一把密鑰驗證通過（常數時間比較）—— 否則 `auth`。
3. `from` 在 `members` 裡、不是本機，且等於 datagram 的來源位址 —— 否則 `sender`。
4. `site` 等於本機 `site` —— 否則 `site`。
5. `|本機時間 − ts| ≤ 5 秒` —— 否則 `stale`。
6. 對該傳送者記錄的 `(boot, seq, ts)`：`boot` 相同時 `seq` 必須大於記錄；`boot` 不同時 `ts` 必須大於記錄 —— 否則 `replay`。通過後更新記錄。

先驗 HMAC 再看內容，未經認證的資料不會影響任何狀態。`boot` 讓傳送者重啟後 `seq` 從 1 重新開始仍被接受；舊 `boot` 的訊息在 5 秒後因 `stale` 被拒，5 秒內因 `ts` 不比記錄新而被拒。5 秒容許的時鐘誤差要求主機有 NTP，文件寫明。

### 密鑰檔每秒最多重讀一次，換密鑰後舊密鑰再簽 60 秒、再接受 120 秒

`secret_file` 規則比照外掛的 `password_file`：絕對路徑，內容去掉一個結尾換行後為密鑰。設定驗證時讀取：讀不到、少於 32 bytes、或檔案權限允許其他使用者讀取（mode & 0o004）皆為設定錯誤。執行期間在每次送心跳前（每秒）檢查檔案，內容變了就換成新密鑰；之後 60 秒內仍以前一把簽章送出，120 秒內仍接受前一把簽的訊息；執行期間讀不到或內容不合規則時沿用最後一把有效的密鑰，每分鐘最多記一行 warn。

各主機的檔案不會同時更新。只要所有主機在 60 秒內先後換完，任兩台之間的訊息都不會被拒：主機 X 在 tX 換，舊密鑰簽到 tX+60；任一台 Y 只要還沒換、或換了不到 120 秒就接受舊密鑰，而 tY ≥ tX−60 保證 tX+60 ≤ tY+120；X 從 tX+60 起改用新密鑰，tY ≤ tX+60 保證 Y 那時已有新密鑰。

修訂紀錄：最初的決定是「換了就立刻用新密鑰簽章、舊密鑰再接受 60 秒」，還沒換的主機會拒收已換主機的訊息，與「通知不中斷」矛盾；verify 發現後改為本決定。

### 每秒心跳，3 秒內收到有效訊息即為存活

每台主機每秒對 `members` 中除自己以外的每一台送一則 `heartbeat`（不論對方是否存活）。最近 3 秒內收到某成員任何一則通過驗證的訊息，該成員即為存活。存活變化記一行 info：`peer 10.0.0.6:5302 is live`、`peer 10.0.0.6:5302 is not live (no valid message for 3s)`。1 秒與 3 秒為估計值（容許連續遺失兩則心跳），本階段不開放設定；存活在本階段只用於 log 與指標，`emfor-peers-split-checks` 才用來重算負責主機。

### 檢查定義摘要

每個服務的摘要為以下 JSON（鍵依字母排序、不含空白）的 SHA-256 前 16 bytes，寫成 32 個小寫 hex：

```json
{"checker":"<checker_id>","options":<check_options，所有表格的鍵依字母排序>,"timeout_ms":<timeout 毫秒>}
```

`checker_id` 即現有的 `ServiceConfig::checker_id`（內建為 `check` 名稱，外掛為 `plugin:<名稱>:<command 的 JSON>`）。只放決定「一次檢查結果代表什麼」的內容：`interval`、`rise`、`fall`、`jitter`、`balance`、後端清單不同不影響一次檢查結果的意義，不放入。TOML 裡鍵的先後順序不同，摘要必須相同。摘要在設定載入時算好，存在 `ServiceConfig`。

摘要不同時該服務不聯防：收到的通知直接忽略（記 debug log）。後兩個 change 也以摘要篩選參與多數決與分工的成員。

### 故障快速通知與快節奏連查

**送出**：後端 task 的檢查造成 `healthy` → `unhealthy` 轉換時，送一則 `down` 給每個其他成員。`unknown` → `unhealthy`（剛啟動）不送，避免整個機房同時重啟時互相觸發。通知只送一次，不重送；UDP 遺失只是失去加速。

**接收**：通過驗證的 `down` 訊息，若本機有該服務、摘要相同、服務有該後端，就交給該後端的 task；否則忽略。後端 task 只在以下條件都成立時開始一段「快節奏連查」：

- 本機該後端目前為 `healthy`；
- 不在另一段快節奏連查中；
- 距離上一段快節奏連查結束已超過 `fall × interval`（冷卻，防止一台設定錯誤的成員讓本機一直快節奏檢查）。

快節奏連查：正在等待時立刻開始下一次檢查；正在檢查時等它結束後立刻開始下一次。之後每次檢查間隔 `min(interval, 200ms)`、不加 jitter，直到後端轉為 `unhealthy`、出現一次 `up`、或這段連查已做滿 `fall` 次為止，然後回到原本的節奏。這幾次檢查的結果照常進 rise / fall 狀態機與 store；通知本身從不改變狀態。

效果（估計，以 tests/peers.rs 的整合測試驗證）：預設 `interval = 2s`、`fall = 3` 時，第一台主機判定後，其他主機約再 `3 × 200ms` 加上檢查耗時就判定，而不是各自最多 6 秒。

開始快節奏連查時記一行 info：`peer 10.0.0.6:5302 reports backend 10.0.0.11:8080 of api.svc.emfor down (connection refused); checking now`。

### 立刻重查通道

每個後端 task 多一個 `Arc<tokio::sync::Notify>`。等待下一次檢查時以 `select!` 同時等 sleep 與 `notified()`；`notify_one` 在 task 檢查中時會留下一個 permit，檢查結束後的等待立刻返回，符合「等它結束後立刻開始」。是否接受通知（上一節的三個條件）由 task 自己判斷，因為只有 task 知道自己的狀態與連查進度。

`Supervisor` 維護一份共享的登記表：服務名稱 → （摘要、後端 id → Notify），在 `sync` 建立或停止 task 時同步更新；聯防模組收到通知時只查這份表。後兩個 change 用同一個通道觸發「負責主機立刻重查」。

### 聯防模組是一個 tokio task，`[peers]` listen 變更需重啟

新增 `peers.rs`：啟動時綁定 `listen` 的 UDP socket（失敗為啟動錯誤，同 DNS 與 metrics），之後一個 task 負責收訊息、送心跳、重讀密鑰、追蹤存活、送出後端 task 交來的通知。後端 task 透過一個無界 mpsc 把「後端轉為 unhealthy」交給它；沒有 `[peers]` 時這個 sender 為 `None`，後端 task 完全不做額外的事。

重載：`members` 與 `secret_file` 透過 `watch` 交給聯防 task，SIGHUP 生效；移除的成員連同其防重送記錄與指標序列一併刪除，新增的成員從「未存活」開始。`[peers] listen` 變更、或新增 / 移除整個 `[peers]`，比照 `[dns] listen` 為重載錯誤 `peers listen settings require restart`。`site` 變更照現有規則於重載生效，訊息的 `site` 欄位隨之改變。

### 指標

有 `[peers]` 時 `/metrics` 多三組：

- `emfor_peer_up{peer="<ip:port>"}`（gauge）：每個其他成員，存活為 1，否則 0。
- `emfor_peer_messages_rejected_total{reason="malformed|auth|sender|site|stale|replay"}`（counter）：六個 reason 一律輸出，從 0 開始。
- `emfor_peer_notifications_total{direction="sent|received"}`（counter）：送出的 `down` 則數（每個收件成員算一則），以及收到且通過驗證的 `down` 則數（不論之後是否被忽略）。

被拒訊息另記 warn，同一來源位址、同一原因每 60 秒最多一行，避免被灌爆 log。

### 三階段完整目標

後兩個 change 照以下內容實作；本 change 只做第一階段。

**第一階段 `emfor-peers`（本 change）**：成員、認證、交換、故障快速通知（A）。每台主機仍完整檢查。

**第二階段 `emfor-peers-quorum`（B，多數決）**：

- 負責主機：每個後端由 k = 3 台負責主機檢查結果做多數決。負責主機以 rendezvous hashing 在「存活且該服務摘要相同」的成員（含本機）中選出：對每個成員算 MD5(`<服務名稱>` + `/` + `<後端 id>` + `/` + `<成員 listen>`) 前 4 bytes big-endian，取最大的 k 台（與 sticky 同一種雜湊）。
- 負責主機每次檢查後送 `kind = "result"`（服務、摘要、後端、`up` / `down`、原因、檢查時間）給所有成員。結果在 3 個 `interval` 內為新鮮。
- 全區判斷：負責主機新鮮結果的多數決。全區判為 down 時優先於本機判斷：本機的組合結果為 `down`，原因 `peers 2/3: <負責主機的原因>`（2/3 為判 down 的票數 / 新鮮結果數）；組合結果仍進 rise / fall。全區判 up 但本機檢查失敗時，只有本機把它判為 down，原因 `local: <原因>`。
- `emfor status` 列出「全區判 down 但本機連得上」的後端，方便找出網路分割或設定差異。
- 退回本機模式：存活且摘要相同的成員（含本機）少於 k 台，或該後端沒有任何新鮮結果時，完全依本機檢查。

**第三階段 `emfor-peers-split-checks`（C，分工）**：

- 只有負責主機做完整檢查；其他主機改做本機 TCP 連線探測，間隔 `interval × 5`，只用來發現「全區 up 但本機連不上」（原因 `local: <原因>`）。
- 成員失聯或恢復時，各主機依自己看到的存活成員重算負責主機；網路分割時兩邊各自在自己的存活成員中重算。
- 退回條件同第二階段；退回時立刻恢復完整檢查。

**共同原則**：查詢只讀本機 store；交換的資料只是本機 daemon 判斷的參考；不跨 `site`、沒有 master；沒有 `[peers]` 時完全是現在的行為。

## Implementation Contract

**行為**

- 沒有 `[peers]`：不綁定任何新 socket、不送任何訊息、`/metrics` 沒有 `emfor_peer_` 開頭的序列，檢查時序與現在完全相同（health-checking 現有測試不需修改即通過）。
- 有 `[peers]`：啟動時綁定 `listen`，失敗則記 error 並以啟動錯誤結束。每秒對每個其他成員送心跳；存活變化記 info。
- 後端 `healthy` → `unhealthy` 時對每個其他成員送一則 `down`；收到符合條件的 `down` 時進入快節奏連查（見 Decisions），開始時記 info。
- 所有驗證失敗的訊息被丟棄、計數，並以每來源每原因每 60 秒一行的頻率記 warn；任何訊息都不會直接改變後端狀態。

**介面與資料格式**

- 設定：`[peers]` 的 `listen`（字串）、`members`（字串陣列）、`secret_file`（字串）；有 `[peers]` 時最上層 `site` 必填。
- 訊息：`EMP1` + 32 bytes HMAC-SHA256 + JSON，欄位如 Decisions 所列，datagram ≤ 1200 bytes。
- 摘要：Decisions「檢查定義摘要」所定義的 32 個 hex 字元，`ServiceConfig` 上的欄位。
- 指標：`emfor_peer_up`、`emfor_peer_messages_rejected_total`、`emfor_peer_notifications_total`。

**失敗模式**

- 設定錯誤（全部一次列出，前綴 TOML 路徑）：`peers.listen` 不是 IP:port、是未指定位址、或不在 `members`；`peers.members` 少於 2 或多於 64 個、有重複或格式錯誤；`peers.secret_file` 缺少、不是絕對路徑、讀不到、少於 32 bytes、其他使用者可讀；`site` 在有 `[peers]` 時缺少；`[peers]` 有其他鍵。
- 重載時 `[peers] listen` 變更或 `[peers]` 新增 / 移除：錯誤 `peers listen settings require restart`，沿用舊設定。
- 執行期間密鑰檔讀不到：沿用最後一把有效密鑰，warn 每分鐘最多一行。
- 送出失敗（例如對方主機不存在）：debug log，不重試。

**驗收**

- crates/emfor-daemon 的單元測試涵蓋：設定驗證表格（每個錯誤一列）、摘要與鍵順序無關、訊息編解碼、六種被拒原因各一例、`boot` 變更後接受、密鑰輪替時雙向訊息都被接受（舊密鑰再簽 60 秒、再接受 120 秒）、快節奏連查的三個停止條件與冷卻、`unknown` → `unhealthy` 不送通知。
- crates/emfor-daemon/tests/peers.rs：兩個 daemon 在 127.0.0.1 不同埠組成群組，檢查同一個由測試控制的後端；後端停止後，第二台在第一台判定後 2 秒內也判定 `unhealthy`（`interval = 5s`、`fall = 3`；沒有通知時兩台可相差將近一個 `interval`）；`/metrics` 顯示對方 `emfor_peer_up` 為 1；停掉一台後 4 秒內變 0。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過；Cargo.lock 不新增套件。

**範圍**

- 範圍內：`[peers]` 設定與驗證、聯防模組（UDP、認證、防重送、存活、心跳、密鑰重讀）、檢查定義摘要、立刻重查通道與快節奏連查、通知的送出與接收、指標、文件（概念、設定、維運、部署、範例、changelog、專案說明）。
- 範圍外：多數決、全區 down 優先、`peers 2/3:` / `local:` 原因、分工、本機 TCP 探測、`emfor status` 的任何改變、LMDB 格式、client、DNS。

## Risks / Trade-offs

- [時鐘誤差超過 5 秒的主機所有訊息被拒] → 文件要求 NTP；`emfor_peer_messages_rejected_total{reason="stale"}` 與 warn log 直接指出問題。
- [密鑰外洩讓攻擊者可偽造 `down` 通知] → 通知只會讓收件主機多做最多 `fall` 次檢查，且有冷卻；本階段任何訊息都無法改變後端狀態。第二階段開始訊息會影響判斷，屆時再評估。
- [UDP 遺失] → 遺失只失去加速，每台主機的判斷仍完全來自自己的檢查。
- [整個機房同時看到大量後端壞掉，通知數為 後端數 × 成員數] → 每則約 200 bytes（估計），1000 個後端 × 63 個成員約 12.6 MB 一次性流量；只在 `healthy` → `unhealthy` 時送一次。尚未實測，實作時以單元測試確認每次轉換只送一輪。
- [各主機 `check_options` 寫法不同導致摘要不同、默默不聯防] → 收到通知被忽略時記 debug log；第二階段的 `emfor status` 列出摘要不同的成員。
- [滾動升級期間新舊版本混用] → 不認得的 `kind` 只更新存活、不計為被拒。

## Migration Plan

1. 產生密鑰（例如 `openssl rand -hex 32 > /etc/emfor/peers.key`，權限 0600 或 0640），發到同一個 `site` 的每台主機。
2. 每台主機設定最上層 `site` 與 `[peers]`（同一份 `members`，各自的 `listen`），開放主機之間的 UDP 5302。
3. 逐台重啟（`[peers]` 新增需重啟）。尚未加入的主機只是收不到通知，不影響其他主機。
4. 回退：移除 `[peers]` 後重啟，即恢復現在的行為。

## Open Questions

- 成員上限 64、心跳 1 秒、存活 3 秒、時間容許 5 秒都是估計值；實際部署後依 `emfor_peer_messages_rejected_total` 與存活變化 log 再調整，本 change 不開放設定。
