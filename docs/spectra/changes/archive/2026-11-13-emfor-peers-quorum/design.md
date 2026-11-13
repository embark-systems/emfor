## Context

emfor-peers（已封存，docs/spectra/changes/archive/2026-11-13-emfor-peers/）完成聯防群組第一階段：`[peers]` 設定、UDP 訊息（`EMP1` + HMAC-SHA256 + JSON）、防重送、每秒心跳與 3 秒存活、檢查定義摘要、故障快速通知與快節奏連查。程式在 crates/emfor-daemon/src/peers.rs（`Group`、`Targets`、`Recheck`、聯防 task `run`）與 schedule.rs 的 `spawn_backend`（每個後端一個 task：檢查 → `BackendHealth::apply` → `WriterMsg::Update`；`healthy` → `unhealthy` 時經 `DownSender` 通知聯防 task）。第一階段的訊息從不改變任何後端的狀態。

該 change 的 design.md「三階段完整目標」已定下第二階段 `emfor-peers-quorum` 的方向：每台後端 k = 3 台負責主機、rendezvous hashing 選出、結果 3 個 interval 內新鮮、多數決、全區 down 優先、原因 `peers 2/3:` / `local:`、`emfor status` 列出不一致。本 change 實作第二階段，並在寫規格時把當時沒定清楚的地方定下來（見 Decisions，其中三項修訂當時的寫法）。

`emfor status` 只讀 store（status.rs 的 `Snapshot`），不連 daemon；所以要讓 status 看到聯防狀態，資料必須寫進 store。store 已有先例：`hist/*`、`meta/metrics_listen`、`meta/dns_listen` 都是 status 專用、client 忽略、`meta/schema_version` 維持 1。

## Goals / Non-Goals

**Goals:**

- 負責主機每次檢查後把結果交給其他成員；每台主機以負責主機的新鮮結果做全區判斷，與本機檢查合成一個結果，再照常進 rise / fall。
- 多數判 down 時全區一致：本機連得上也判 down，原因標明來自哪幾票。
- 維運人員從 `emfor status`、`/metrics` 與 log 看到成員存活、摘要不同的成員、全區判 down 但本機 up 的後端。
- 成員不足、沒有新鮮結果、沒有 `[peers]` 時行為與現在完全相同。

**Non-Goals:**

- 分工檢查、本機 TCP 探測（`emfor-peers-split-checks`）。本階段每台主機仍完整檢查每台後端。
- 可設定的 k、新鮮時間、票數門檻：本階段固定，部署後依實際情況再議。
- 讓 client 或 DNS 讀聯防資料；改變 `svc/*` 服務記錄或 `hist/*` 的格式。
- 跨 `site`、master / 選主、加密訊息內容（與第一階段相同）。

## Decisions

### 訊息種類 result：負責主機每次 up / down 檢查後送出

新增 `kind = "result"`，除共同欄位外帶 `service`、`digest`、`backend`、`outcome`（`"up"` 或 `"down"`）、`reason`（只在 `down` 時有，UTF-8 不超過 256 bytes，在字元邊界截斷）、`interval_ms`（傳送者該服務的 `interval`，毫秒）。缺欄位、`outcome` 不是這兩個值、`interval_ms` 不是正整數：`malformed`。

本機是某後端的負責主機時，每次本機檢查結果為 `up` 或 `down` 就送一則給每個其他成員；`error` 不送（`error` 不代表後端狀態，與 rise / fall 的處理一致）。送的是**本機檢查的原始結果**，不是合成後的結果，避免全區判斷互相放大。

替代方案：只在結果改變時送、加上定期補送 —— 送的則數較少，但遺失一則就要等補送，新鮮時間也要跟補送週期綁在一起。估計（未實測）每台主機每秒送出與收到的 `result` 都約為 `3 × 後端數 ÷ interval` 則，與成員數無關：1000 台後端、`interval = 2s` 約 1500 則、每則約 250 bytes，約 375 KB/s；可接受，故選每次都送。實作後以整合測試的 `emfor_peer_results_total` 確認則數符合公式。

### 負責主機的選法：存活成員加本機，rendezvous hashing 取前 3

**Supersedes**: emfor-peers / 三階段完整目標

候選為「本機 + 目前存活的其他成員」。對每個候選算 MD5(`<服務名稱>/<後端 id>/<成員 listen>`，UTF-8)，取前 4 bytes 當 big-endian 無號整數，最大的 3 台為負責主機；相同時 listen 字串較小者勝。成員 listen 用設定檔的寫法（IPv6 為 `[addr]:port`）。候選少於 3 台時該後端不做多數決（退回本機模式）。

與當時寫法的差異：當時寫「存活且該服務摘要相同」的成員。但摘要只在對方送來 `down` 或 `result` 時才知道，不是負責主機的成員從不送 `result`，本機無從得知它的摘要，選法會依賴先後順序、各主機算出不同的負責主機。改成不看摘要：摘要不同的負責主機送來的結果在計票時丟掉（並記為摘要不同，顯示在 status），結果是票數變少、退回本機模式，而不是各主機選出不同的負責主機。

選法用的 MD5 與 sticky 的 rendezvous hashing 相同（emfor-daemon 新增相依 md-5 0.11，Cargo.lock 已有）。每次合成結果時重算：最多 64 個候選、各一次 MD5，估計每次數微秒，不快取。

### 全區判斷：至少 2 票 down 才判全區 down

**Supersedes**: emfor-peers / 三階段完整目標

一票是某台負責主機對該後端的最新結果：本機是負責主機時為本機這次的檢查結果（`error` 不算票）；其他負責主機則是最近收到的 `result`，必須同時符合：摘要等於本機該服務目前的摘要、收到時間在 `3 × interval_ms`（傳送者的 interval）以內、傳送者目前存活。

- 新鮮票數為 0：退回本機模式。
- down 票數 ≥ 2（k = 3 的過半數）：全區判 down。
- 其他：全區不判 down。

與當時寫法的差異：當時寫「新鮮結果的多數決」，代表只剩 1 票新鮮且為 down 時，一台主機就能讓全區判 down。改成固定需要 2 票，一台負責主機（或其網路路徑）出問題不會拖垮全區。新鮮時間改用傳送者的 `interval`：`interval` 不在摘要裡，各主機可以不同，用接收者自己的 `interval` 會讓檢查較慢的負責主機的票永遠過期。

### 合成結果與原因前綴

**Supersedes**: emfor-peers / 三階段完整目標

每次本機檢查後算一次合成結果，取代原本直接進狀態機的檢查結果：

| 全區判斷 | 本機結果 | 合成結果 | 原因 |
| --- | --- | --- | --- |
| 退回本機模式 | 任何 | 本機結果 | 本機原因（不加前綴） |
| 全區 down（d 票 / f 票新鮮） | 任何 | `down` | `peers d/f: <第一張 down 票的原因>` |
| 不判 down | `up` | `up` | — |
| 不判 down | `down` | `down` | `local: <本機原因>` |
| 不判 down | `error` | `error` | 本機原因（不加前綴） |

「第一張 down 票」依負責主機的排名（雜湊值大者在前）。原因加前綴後再套用現有的原因正規化與 512 字元上限。合成結果是唯一往下游走的結果：rise / fall、store 的 `state` / `reason`、`hist/*`、狀態變化 log、`emfor_backend_checks_total{result}`、快節奏連查的停止條件（遇到 `up` 停）、第一階段的 `down` 通知（任何 `healthy` → `unhealthy` 轉換）。延遲樣本仍來自本機檢查，只在合成結果為 `up` 時計入（與現在「只有 up 檢查計入」相同）。

當時寫法「全區判 up 但本機檢查失敗時原因為 `local:`」細化為上表：只有在做多數決（至少一張新鮮票）且全區不判 down 時加 `local:`；退回本機模式時原因與現在完全相同，沒有聯防群組的使用者看到的不變。

### 不一致與摘要不同的追蹤

聯防模組維護兩份清單，供 `meta/peers`、status、指標使用：

- **不一致後端**：最近一次合成時全區判 down、而本機結果為 `up` 的後端，記下 down 票數與新鮮票數；下一次合成不再符合、或後端 / 服務被移除時刪除。開始不一致時記一行 info：`backend <id> of <service> is down at <d>/<f> peers but up here`。
- **摘要不同**：收到某成員對本機有的服務送來的 `down` 或 `result`，摘要與本機不同時記下（服務、成員）與時間；60 秒內沒再收到就移除，成員被移除時一併移除。60 秒為估計值，只影響顯示多久。

### meta/peers：聯防狀態寫進 store 給 status 讀

有 `[peers]` 時 daemon 寫 key `meta/peers`，值為 JSON：

```json
{"listen":"10.0.0.5:5302",
 "members":[{"addr":"10.0.0.6:5302","live":true},{"addr":"10.0.0.7:5302","live":false}],
 "digest_mismatch":[{"service":"api.svc.emfor","member":"10.0.0.7:5302"}],
 "disagreements":[{"service":"api.svc.emfor","backend":"10.0.0.11:8080","down":2,"fresh":3}]}
```

`members` 為設定順序、不含本機；兩份清單依服務名稱、再依成員或後端 id 排序。啟動時與 `meta/started_at` 同一交易寫入（全部成員 `live: false`、清單為空）；沒有 `[peers]` 時同一交易刪除。之後聯防 task 每秒檢查一次，內容改變才交給 writer，writer 於下一次 commit 寫入。`meta/schema_version` 維持 1，client 與 DNS 不讀。

替代方案：在 `svc/*` 的後端記錄加欄位 —— 改到每個語言 client 都要解析的格式；status 連 daemon 取資料 —— 違反「不做 HTTP API」且 status 目前完全不連 daemon。

### emfor status 顯示聯防群組

daemon 狀態為 running 且 `meta/peers` 可解析時，daemon 那行之後多一行 `peers: <存活數>/<其他成員數> live`，有未存活成員時接 `, not live <addr> <addr>…`（設定順序）；每個（受 `--service` 篩選後的）摘要不同的服務一行 `peers: check definition of <service> differs at <member> <member>…`。不一致後端的那行在 `pool` 標記的位置之後加標記 `local-up`。`--json` 多一個 `peers` 欄位：`meta/peers` 解析後的值，key 不存在或不是 JSON 物件時為 `null`（不受 `--service` 篩選，整份輸出）。結束碼不變：全區判 down 已經讓後端變成 unhealthy，`all_down` 照原規則決定。

### 指標

有 `[peers]` 時 `/metrics` 多：

- `emfor_peer_results_total{direction="sent"}` / `{direction="received"}`（counter）：送出的 `result` 則數（每個收件成員算一則）、收到且通過驗證的 `result` 則數（不論之後是否計票）。
- `emfor_peer_disagreements`（gauge）：目前不一致後端的數量。

### 程式結構

- 新模組 crates/emfor-daemon/src/quorum.rs：`rendezvous()`、`assigned()`（負責主機）與 `Quorum`（`Arc` 共享，內部 `Mutex`）：本機 listen、存活成員集合、票表（服務、後端 → 成員 → 結果、原因、收到時間、`interval_ms`、摘要）、不一致與摘要不同清單。`Quorum::combine` 回傳合成結果、本機是否為負責主機與是否不一致；`summary` 產生 `meta/peers` 內容；`retain` 依目前的服務與後端清除票表。
- peers.rs 新增 `result` 訊息（`ResultMsg`、`Outcome`）。聯防 task 在收到 `result` 時寫票表、在存活集合改變時更新 `Quorum`、每秒經 `StateOut` 把 `meta/peers` 內容交給 writer（內容沒變時不送）。
- 後端 task 對外只拿一個 `PeerHooks { out, quorum, targets }`：`out` 是 `PeerSender`，沿用同一條無界 mpsc，送 `PeerOut::Down`（down 通知）或 `PeerOut::Result`（`result` 訊息）。`PeerLink` 帶 `Option<PeerHooks>`（沒有 `[peers]` 時為 `None`，後端 task 完全照原本的路徑走）；schedule.rs 的 `combine` 在每次檢查後呼叫 `Quorum::combine`，並在本機為負責主機且結果為 up / down 時送出 `result`。
- 摘要與後端清單仍由 `Targets` 提供；`Supervisor::publish_targets` 之後呼叫 `Quorum::retain`，同步刪除票表與不一致清單中已不存在的服務與後端。
- writer 新增 `WriterMsg::PeerState` 帶 `meta/peers` 的新內容，並記住最新值；`ListenKeys.peers` 是啟動時的初始內容。啟動交易的寫入 / 刪除與 `meta/metrics_listen` 同一處理方式，重新開始（`fresh_start`）時寫入記住的最新值。

## Implementation Contract

**行為**

- 沒有 `[peers]`：不送 `result`、合成結果等於本機結果、store 沒有 `meta/peers`、status 沒有 `peers:` 行、`/metrics` 沒有 `emfor_peer_` 序列；health-checking、state-store、status-command 現有測試不需修改即通過。
- 有 `[peers]`：負責主機每次 `up` / `down` 檢查後對每個其他成員送一則 `result`；每次檢查依「合成結果與原因前綴」的表格產生合成結果，進 rise / fall 與所有下游。
- 存活成員加本機少於 3 台、或新鮮票數為 0：合成結果等於本機結果，原因不加前綴。
- 全區判 down 優先於本機 `up`；該後端出現在 `meta/peers` 的 `disagreements`、status 標 `local-up`、`emfor_peer_disagreements` 計入，並記一行 info。

**介面與資料格式**

- 訊息：`kind = "result"`，欄位 `service`、`digest`、`backend`、`outcome`、`reason`（僅 down）、`interval_ms`。
- 負責主機：MD5(`<service>/<backend id>/<member listen>`) 前 4 bytes big-endian，取最大 3 台，同值 listen 字串小者勝。
- 原因：`peers <d>/<f>: <reason>`、`local: <reason>`。
- store：`meta/peers` JSON，形狀如 Decisions 所列；`emfor_format::KEY_PEERS = "meta/peers"`。
- status：`peers: <n>/<m> live[, not live …]`、`peers: check definition of <service> differs at …`、後端標記 `local-up`、JSON 欄位 `peers`。
- 指標：`emfor_peer_results_total{direction}`、`emfor_peer_disagreements`。

**失敗模式**

- `result` 欄位不全或值不合法：計為 `malformed` 丟棄（沿用第一階段的 warn 頻率限制）。
- 對本機沒有的服務或後端的 `result`：計入 received、不計票、不記錄。
- 摘要不同：不計票，記入摘要不同清單，debug log。
- `meta/peers` 寫入失敗：與其他 key 相同，下一次 commit 重試，不影響檢查。
- 送出失敗：debug log，不重試（同第一階段）。

**驗收**

- 單元測試：`result` 編解碼與 malformed 情形；負責主機選法（規格中的雜湊範例）；計票表格（退回本機、2/3、1/3、摘要不同、過期、傳送者不存活）；合成結果表格每一列；不一致清單的加入與移除；`meta/peers` JSON 形狀；status 的 `peers:` 行、`local-up` 標記與 JSON `peers`；指標輸出。
- crates/emfor-daemon/tests/peers.rs：三個 daemon（127.0.0.1 不同埠）以 exec 檢查同一台後端，檢查指令相同、結果由各 daemon 行程自己的環境變數指向的旗標檔決定。兩台判 down、一台 up 時，第三台在 `fall` 次檢查內把後端判為 `unhealthy`，原因以 `peers 2/` 開頭，store 的 `meta/peers` 列出不一致，`emfor status` 印出 `local-up`；只有一台判 down 時，其他兩台維持 `healthy`，判 down 那台原因以 `local: ` 開頭。兩個 daemon 的群組中後端狀態完全依本機（退回本機模式）。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過；Cargo.lock 不新增套件。

**範圍**

- 範圍內：`result` 訊息、負責主機、計票與合成結果、不一致與摘要不同追蹤、`meta/peers`、status 顯示、兩個新指標、文件（概念、設定、維運、部署、changelog、專案說明、LANGUAGE.md）。
- 範圍外：分工檢查與本機 TCP 探測、可設定的 k / 門檻 / 新鮮時間、client 與 DNS、`svc/*` 與 `hist/*` 格式、設定檔語法。

## Risks / Trade-offs

- [密鑰外洩可偽造 `result`，讓全區把後端判 down] → 第一階段的風險評估寫明「第二階段開始訊息會影響判斷，屆時再評估」：本階段仍只依賴共用密鑰；攻擊者需同時持有密鑰並能從成員位址送出 UDP（`sender` 檢查比對來源位址，同一機房內偽造來源位址仍可能）。文件要求密鑰權限 0600 / 0640，並說明移除 `[peers]` 即可回到本機模式。
- [本機連得上但被判 down，該主機上的應用少一台後端] → 這是全區一致的代價，也是「全區 down 優先」的目的；status 的 `local-up`、`emfor_peer_disagreements` 與 info log 讓維運人員看到。
- [滾動升級時第一階段的主機被選為負責主機但從不送 `result`] → 票數變少；3 台負責主機中有 1 台舊版仍可湊到 2 票，全部升級前可能較常退回本機模式，不會誤判。
- [各主機看到的存活成員不同，選出不同的負責主機] → 每台只計自己認定的負責主機的票，最壞是票數少而退回本機模式。
- [`result` 訊息量隨後端數成長] → 估計見 Decisions，未實測；整合測試以 `emfor_peer_results_total` 確認每次檢查只送一輪。
- [各主機時鐘差] → 新鮮時間用本機收到的時間（單調時鐘）計算，不比較傳送者的時間；時鐘差只影響第一階段已有的 `stale` 檢查。

## Migration Plan

1. 逐台升級（重啟）。升級期間新舊版混用只會讓票數變少，不會誤判。
2. 全部升級後多數決自動生效，不需改設定。
3. 回退：降回第一階段版本，或移除 `[peers]` 後重啟。`meta/peers` 在沒有 `[peers]` 的啟動時被刪除；舊版不認得它，也不會刪除，status 舊版不讀它。

## Open Questions

- k = 3、2 票門檻、`3 × interval` 新鮮時間、摘要不同保留 60 秒都是估計值，本 change 不開放設定；部署後依 `emfor_peer_disagreements` 與 info log 再議。
