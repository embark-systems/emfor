## Context

- `peers/group.rs` 的 `Group::accept` 呼叫 `check`；`check` 依序做格式與密鑰驗證、寄件者（`msg.from == src` 且是成員）、`site`，然後才比 `now_ms.abs_diff(msg.ts) > MAX_SKEW_MS`（5000）回 `Err(Reject::Stale)`，最後是重放檢查。所以到 `stale` 判斷時，時間戳來自已驗證、同 `site` 的成員，是可信的。
- `check` 目前只回 `Reject`；`Err(Stale)` 時解碼出來的 `msg` 被丟掉，`accept` 只能計數與 `warn_rejected`（每個來源與原因 60 秒一次）。被判 `stale` 的訊息不會設 `seen`，所以時鐘差超過 5 秒的成員永遠不是存活的成員。
- 每個成員的狀態在 `Group.peers: HashMap<SocketAddr, PeerState>`（`last`、`seen`、`live`、`v3`）。每秒的 peer tick（`peers/task.rs`，`HEARTBEAT_EVERY`）呼叫 `group.check_liveness`、`quorum.update_silent`，再把 `quorum.summary(...)` 經 `StateOut::publish` 寫成 `meta/peers`（值有變才寫）。
- 上一個變更（沉默的負責主機）已經建立了範本：`Summary` 的選用欄位用 `skip_serializing_if = Vec::is_empty`，`status.rs` 讀端用 `#[serde(default)]`，`PeerStats` 放給 metrics 讀的原子值，進入記 `warn`、離開記 `info` 各一次。
- `emfor_peer_up{peer="<addr>"}` 是目前唯一的每成員 metric；它的 label 沒有跳脫，新的 metric 要用 `metrics.rs` 的 `label()`。

## Goals / Non-Goals

**Goals:**

- 在偏差到達 5 秒之前（2 秒起）讓維運看到哪個成員的時鐘差多少、往哪個方向。
- 時鐘差已超過 5 秒、成員的訊息全被丟掉時，仍然量得到偏差。
- 分得出「某一個成員的時鐘錯」與「本機的時鐘錯」。
- 不改協定、不改 5 秒限制、不改存活、投票與排名。

**Non-Goals:**

- 不校正時鐘、不補償偏差；不新增設定項；不量網路延遲。
- 不偵測 client 端與 heartbeat 落在未來的情況（`emfor status` 已有）。
- 不改既有 `stale` 計數與 warn。

## Decisions

### 在 `check` 回傳偏差，由 `accept` 記進成員狀態

`check` 在通過 `site` 檢查之後、判斷 `stale` 之前，算出 `offset_ms = msg.ts - now_ms`（`i64`，正＝成員比本機快）。`Err(Stale)` 時要把這個值帶出來：`check` 的錯誤型別改成帶選用偏差的小型別（例如 `Rejected { reason: Reject, offset_ms: Option<i64> }`，只有 `site` 之後才有值；其他 `Reject` 的 `offset_ms` 是 `None`），`accept` 在兩條路徑都把偏差寫進 `PeerState` 的 `clock: Option<ClockSample { offset_ms, at: Instant }>`。理由：時間戳只有在 `check` 內可信且可得，不重新解碼；替代方案是在 `accept` 先解碼一次再呼叫 `check`，會重複驗證密鑰，不採用。

重放的訊息不量：`check` 在記偏差之前先問該成員已接受過的最新狀態（`Last::admits`），重送一個已被接受過的訊息時不設偏差。理由：重送的訊息時間戳已經很舊，拿到一則被擷取的 datagram 的人可以重送幾次，讓某個成員的時鐘看起來錯了；review 發現這一點後把規格從「重放也量」改成「重放不量」。成員從未被接受過任何訊息、或訊息是第一次被收到的亂序舊訊息時，仍然會量。

### 量測的壽命

一筆量測保留 10 秒（`CLOCK_KEEP`），期間沒有再收到任何通過 `site` 檢查的訊息就丟掉（成員停了、被切斷、或停止送）。丟掉量測時，若成員處於時鐘偏差狀態就離開它（記 `info`）。量測與存活無關：時鐘差超過 5 秒的成員不存活，但量測仍有效。

### 門檻、連續與遲滯

進入：最近連續三則訊息的偏差絕對值都達到 2000 毫秒（`CLOCK_WARN_MS`），且同一個符號。離開：任何一則訊息的絕對值低於 1500 毫秒（`CLOCK_CLEAR_MS`），或量測過期。介於兩者之間：維持原狀態。理由：偏差含單向延遲，一個被延遲的 datagram 會顯得像成員慢了；`tools/peer-wan-test.sh` 看過延遲到 2 秒的情況，所以進入需要連續三則（在 1 秒一次的 heartbeat 下約 3 秒），避免單一延遲誤報；遲滯避免在 2 秒邊界來回跳。這些值是設計選擇，不是量出來的：用單元測試固定行為，用 docker 撥動時鐘確認 2 秒起 3 到 5 秒內出現預警。

### 狀態與 log 放在 `Group`，每秒掃一次

`Group::update_clock(now: Instant)` 在每秒的 tick 裡執行（`check_liveness` 之後）：丟掉過期量測、套用上面的進出規則。進入記一次 `warn`：`clock of peer <member> is <n.n> s behind|ahead of this host; messages more than 5 s apart are dropped`；離開記一次 `info`：`clock of peer <member> is within limits again`。每個成員各一次；訊息數量不影響 log 量。偏差與「是否處於偏差狀態」的連續計數存在 `PeerState`，進入／離開由 `update_clock` 以外的路徑改變時（`accept` 只記量測與計數，不記 log），log 只由 `update_clock` 發出，所以每個轉換只記一次。

### 輸出：全部成員都偏差時指向本機

- `Summary`（`quorum.rs`）新增 `clock_offsets: Vec<SummaryClock>`（`member`、`offset_ms`），只含處於偏差狀態的成員，依成員位元組序排序；空的時候不寫（`skip_serializing_if`）。`Quorum::summary` 目前不認識 `Group` 的狀態，所以 tick 把 `Group` 的清單傳進去（簽章加一個參數，或 `Summary` 組好之後再填欄位，由實作選擇較小的改動）。
- `emfor status`：處於偏差狀態的成員各一行 `peers: clock of <member> is <n.n> s behind|ahead of this host (messages more than 5 s apart are dropped)`，放在沉默 checker 的行之後。`<n.n>` 取偏差絕對值，四捨五入到一位小數；成員比本機慢是 `behind`，快是 `ahead`。當 `status` 看到的 `clock_offsets` 筆數等於 `meta/peers.members` 的筆數（`members` 列的是本機以外的成員）、且至少兩個，就把這些行換成一行 `peers: clock of this host differs from every member by up to <n.n> s; check this host's clock (messages more than 5 s apart are dropped)`，`<n.n>` 取最大的絕對值。只有一個其他成員時沒辦法判斷是誰錯，用逐成員的行。理由：本機的時鐘錯時，所有成員都會顯示偏差，逐一列出會誤導維運去查每個成員。
- metric `emfor_peer_clock_offset_seconds{peer="<addr>"}`：gauge，秒，帶正負號，每個目前有量測的成員一筆（無論是否處於偏差狀態），label 用 `label()` 跳脫；沒有 `[peers]` 時不輸出。值由 tick 存進 `PeerStats`（成員清單固定，最多 64 個），沒有量測的成員不輸出。

## Implementation Contract

**行為：**

- 成員 C 的時鐘比 A 慢 3 秒：A 在三則訊息（約 3 秒）後 `emfor status` 出現 `peers: clock of <C> is 3.0 s behind this host (messages more than 5 s apart are dropped)`，`status --json` 的 `peers.clock_offsets` 有 `{"member":"<C>","offset_ms":-3000}`（約，視實際量測），`emfor_peer_clock_offset_seconds{peer="<C>"}` 約為 -3，log 有一次 `warn`。
- C 的時鐘慢到 6 秒：訊息被丟（`stale` 計數上升、既有 warn 照舊），但偏差仍被量到並顯示為 `behind` 約 6.0 秒；A 與 C 的存活判斷與投票不變。
- 時鐘修正（偏差低於 1.5 秒）：下一則訊息後離開狀態，status 行與 JSON 欄位消失，log 一次 `info`。
- A 的時鐘錯、B 與 C 看起來都偏差：status 只出現一行指向本機。
- 沒有偏差時 `meta/peers`、`status` 與 metrics 的其他輸出與現在相同，只多 metric 的 `emfor_peer_clock_offset_seconds` 系列。

**資料形狀：** `meta/peers.clock_offsets`: `[{"member": "<ip:port>", "offset_ms": <整數，帶符號>}]`；選用，缺少當空。`check` 的錯誤型別帶 `offset_ms: Option<i64>`。常數：`CLOCK_WARN_MS = 2000`、`CLOCK_CLEAR_MS = 1500`、`CLOCK_CONSECUTIVE = 3`、`CLOCK_KEEP = 10 s`。

**錯誤與靜默：** 量測不會失敗；沒有量測的成員不輸出 metric、不出現在 JSON。log 只在狀態轉換時發出。時間全由傳入的 `now_ms`／`now: Instant` 控制，單元測試可以操縱。

**驗收：** group.rs 單元測試（符號、三則連續、遲滯、`stale` 訊息也量到、過期、log 一次）；status.rs 文字行與「全部成員都偏差」的測試；metrics 輸出測試；docker 驗證（撥動容器時鐘，若做不到則在手冊註明只有單元測試）；`tools/docker-test.sh` 全部通過。

**範圍：** 在範圍內：peer-group、status-command、state-store、metrics-endpoint 四份 spec、daemon 程式碼、手冊中英、CHANGELOG、LANGUAGE.md、一個驗證腳本。範圍外：協定、`select.rs`、設定項、client 端偵測、時鐘校正。

## Risks / Trade-offs

- **偏差含單向延遲**：跨機房的群組延遲可達秒級，會顯得像時鐘差；手冊第 11 章已說明不建議跨機房，這次不處理，只在手冊說明偏差是「時間戳差」不是校準過的時鐘差。
- **三則連續與 2 秒是估計值**：沒有量過真實環境的時鐘分布；用 docker 驗證是否合理，需要時再調常數，不新增設定項。
- **`check` 錯誤型別的改動面**：所有 `Reject` 的回傳點都要改；用輔助建構子（`Rejected::from(Reject)`）讓大部分回傳點維持一行。
- **單一其他成員時沒辦法判斷誰錯**：兩個成員的群組中，一方偏差時雙方都會顯示對方偏差；status 行寫「of <member>」，手冊說明兩台的情況。
