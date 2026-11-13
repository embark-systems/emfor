## Context

聯防群組前兩階段已完成（emfor-peers、emfor-peers-quorum，皆已封存）：成員以 UDP 互送心跳與 `down` 通知；每台後端由 3 台負責主機（rendezvous hashing 在本機與存活成員中選出）於每次檢查後送出 `result`，各主機在每次自己的檢查後用 quorum.rs 的 `Quorum::combine` 把本機結果與負責主機的新鮮結果合成。目前群組中每台主機仍在每個 `interval` 完整檢查每台後端，N 台 daemon 使每台後端每個 `interval` 被檢查 N 次。

emfor-peers 的 design.md 規劃第三階段讓非負責主機改做本機 TCP 探測。emfor-check 的 DNS、RADIUS、SIP 常走 UDP，TCP 探測會把正常的後端判 down；使用者決定改為：非負責主機照服務原本的檢查方式檢查，只是至少每 `interval × 5` 一次。

## Goals / Non-Goals

**Goals:**

- 非負責主機在負責主機的新鮮結果已能決定時不自己檢查，每台後端的檢查次數由每 `interval` N 次降到約 3 + (N − 3) / 5 次（估計值，以整合測試中各 daemon 的檢查次數確認）。
- 全區 up 但本機連不上時，非負責主機仍在至多 `interval × 5` 內發現，並以 `local: <原因>` 判 down。
- 投票不足以決定、成員不夠、本機是負責主機、快節奏連查時，完全照第二階段的行為。

**Non-Goals:**

- 本機 TCP 連線探測（使用者已否決）。
- 可設定的倍數或負責主機數；`× 5` 與 3 都固定。
- `emfor status` 的任何改變、`meta/peers` 格式改變。
- 設定檔語法、LMDB 格式、client、DNS 的任何改變。

## Decisions

### 非負責主機沿用原本的檢查方式，間隔 interval × 5

選擇：非負責主機自己檢查時用的就是服務設定的 `check`、`check_options`、`timeout`，只是連續 4 次靠投票決定後第 5 次一定自己檢查。替代方案「TCP 連線探測」對 UDP 服務會誤判，「TCP 探測但 UDP 服務不分工」需要每種檢查方式標明傳輸協定，外掛也要回報；兩者都被使用者否決。以「次數」而非「時間」計算第 5 次，跟現有排程（每次檢查後依結果決定下一次等待）自然結合，抖動與快節奏規則不用改。

### 投票決定的條件：至少 2 票同向

選擇：非負責主機在 `d ≥ 2` 時判 down、`u ≥ 2` 時判 up，否則自己檢查。2 是 3 台負責主機的多數，與第二階段「至少 2 票 down 判全區 down」同一門檻。只有 1 票、或 1 down 1 up 時投票無法代表多數，必須自己檢查；這也涵蓋負責主機剛重算、結果還沒收到的時候。

### 本機結果不是 up 時改回每次自己檢查

選擇：非負責主機最近一次自己檢查的結果不是 `up`（`down` 或 `error`）時，之後每次都自己檢查，直到結果回到 `up`。這讓 `fall` 次連續的本機失敗能在 `fall` 個 `interval` 內累積成 `local:` 的 down，與第二階段相同；若沿用最後一次本機結果到投票時間，單一次失敗會被重複計算成多次，若投票時間改判 up，則永遠累積不到 `fall`。

### 沒有延遲值時不靠投票判 up

選擇：投票會判 `up`、但後端目前沒有延遲值（剛啟動、或因投票轉為 unhealthy 而被清空）時，本機改為自己檢查。否則後端可能只靠投票回到 healthy 卻沒有延遲值，`balance = "fastest"` 的快速池（只算有延遲值的 healthy 後端）會把它排除，直到下一次自己檢查（最多 4 次檢查時間）。投票判 `down` 時不受影響，全區故障期間仍可略過檢查。

### 投票決定的那一次與一般檢查的差別

選擇：投票決定的結果照常進狀態機、store、state history、log、`emfor_backend_checks_total`，等待下一次的規則也相同；但不量延遲（延遲只在本機 up 檢查時更新，轉為 unhealthy 時照舊清空）、不送 `result`（不是自己的檢查）、不送 `down` 通知（負責主機已經知道，避免 N − 3 台各送 N − 1 則）、不改變不一致狀態（不一致的定義需要本機 up 的結果）。

### 程式結構

- quorum.rs 新增 `Quorum::votes(service, backend, digest, now) -> Votes`：回傳本機是否為負責主機、是否有負責主機、其他負責主機的新鮮票數 `f`、`down` 票數 `d`、`up` 票數 `u` 與排名最高的 `down` 原因；與 `combine` 共用同一段取票邏輯（抽成內部函式），不改變 `combine` 的結果。另有 `Quorum::skip()` 累計略過次數與 `skipped()` 讀取。
- schedule.rs 新增純函式 `fn split(votes: &Votes, last_own_up: Option<bool>, has_latency: bool, decided_in_row: u32, in_fast_run: bool) -> Step`（`last_own_up` 為 `None` 表示還沒自己檢查過），`Step` 為 `Check` 或 `Decided(CheckResult)`；後端 task 在每次排定時間先呼叫它（只在有 `PeerHooks` 時），`Check` 走原本的檢查與 `combine`，`Decided` 直接以該結果進狀態機。後端 task 自己記 `last_own_up`、`decided_in_row`，`has_latency` 取自目前的延遲值。
- peers.rs 的每秒 tick 把 `quorum.skipped()` 複製到 `PeerStats` 的新欄位（與 `disagreements` 相同做法），metrics.rs 輸出 `emfor_peer_checks_skipped_total`。

## Implementation Contract

**行為**

- 沒有 `[peers]`、或後端沒有負責主機（存活成員連同本機少於 3 台）、或本機是負責主機：每次排定時間都自己檢查，行為與第二階段完全相同，現有測試不需修改即通過。
- 非負責主機：依 peer-group 規格「Split checks」的條件決定自己檢查或以投票決定；投票決定時 `d ≥ 2` 為 `down`（原因 `peers <d>/<f>: <原因>`），否則為 `up`。
- 投票決定的那一次：進狀態機、store、history、log、檢查計數；不量延遲（轉為 unhealthy 時延遲照舊清空）、不送 `result`、不送 `down`、不改不一致；`emfor_peer_checks_skipped_total` 加 1。
- 等待下一次的規則（抖動、狀態未知時的 200ms 快節奏、快節奏連查）不變，投票結果視同檢查結果。

**介面與資料格式**

- 指標：`emfor_peer_checks_skipped_total`（counter，無標籤，只在有 `[peers]` 時輸出，起始 0）。
- 沒有新的訊息種類、設定鍵或 store key。

**失敗模式**

- 負責主機失聯：其結果在 `3 × interval_ms` 後不再新鮮、或傳送者不再存活即不算票；票數不足時本機立刻自己檢查。
- 摘要不同：對方結果不算票（第二階段規則），票數不足時自己檢查。
- 時鐘與 UDP 遺失：同前兩階段；遺失只讓本機多做檢查。

**驗收**

- quorum.rs 單元測試：`Quorum::votes` 對 peer-group 規格「what this daemon does at one check time」範例中的票數與原因；`combine` 既有測試不變仍通過。
- schedule.rs 單元測試：`split` 依範例表逐列；後端 task 在暫停時間下重現規格情境「Sparse own checks while the assigned checkers see the backend up」（10 次中第 1、6 次自己檢查）、「Local failure while the region is up」、「Region down without an own check」、「Moves decided by votes are not announced」，以及 health-checking「Check times decided by votes keep the schedule」。
- metrics.rs：`emfor_peer_checks_skipped_total` 出現並等於 quorum 累計值；沒有 `[peers]` 時沒有此序列。
- crates/emfor-daemon/tests/peers.rs：四個 daemon 組成群組檢查同一個 exec 後端，每個 daemon 的檢查次數寫入各自的計數檔；數秒後非負責的那台的檢查次數明顯少於負責主機（少於其一半），停掉一台負責主機後，剩下的 3 台都成為負責主機，原本非負責的那台恢復每次檢查。另一個情境：非負責主機的旗標檔讓本機檢查失敗時，它以 `local: ` 判 unhealthy，其他三台維持 healthy。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過；Cargo.lock 不變。

**範圍**

- 範圍內：分工檢查的判斷、後端 task 排程、略過計數指標、文件（概念、設定、維運、部署、changelog、專案說明、LANGUAGE.md 加「投票決定」）。
- 範圍外：TCP 探測、可設定倍數、`emfor status` 與 `meta/peers`、訊息格式、client、DNS。

## Risks / Trade-offs

- [非負責主機最多晚 `interval × 5` 才發現只有本機連不上的故障] → 這類故障 client 讀的是本機 store，本機會繼續把流量導向那台後端最多 `5 × interval + fall × interval`；全區故障則仍由投票在 `fall` 次內判 down。文件說明這個取捨。
- [負責主機剛重算時票數不足，短時間內非負責主機全部自己檢查] → 只持續到新的負責主機送出第一輪 `result`（約一個 `interval`），之後恢復分工。
- [投票決定的 up 讓延遲數值停在上一次本機檢查] → 延遲每 `interval × 5` 至少更新一次；文件註明。
- [估計的檢查次數下降尚未實測] → 整合測試記錄各 daemon 的檢查次數作為確認。
