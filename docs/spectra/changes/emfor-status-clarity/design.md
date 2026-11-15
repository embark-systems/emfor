## Context

`emfor status` 的文字輸出由 `crates/emfor-daemon/src/status.rs` 產生：`services_lines` 逐 backend 組出一行（五個欄位 id、state、checked、latency、pool，之後依序附上 `local-up`、`voted`、`FLAPPING` 標記與 `reason`），`peer_lines` 產生 `peers:` 各行。輸出由單元測試逐字比對，spec `status-command` 的「Status text output」要求列出每個元素。

值班評估（見 proposal 的 Why）找出三個缺口，都能只靠 store 已有的資料補上：

- `Backend.backup`（emfor-format）在 store 裡，文字輸出沒用到。
- `Backend.reason` 的文件註解是「最近一次 `down` 或 `error` 結果的訊息，`up` 之後為 `None`」；手冊第 5 章說明 `healthy` 且 `reason` 有值就代表最近一次檢查失敗，而且只要別的 healthy backend 還通過檢查，它就已被排除在候選者之外。
- `assigned_checkers` 已經算好每個顯示的 backend 的 assigned checker；`quorum::assigned` 在候選者（本機加上 live 且沒有略過該服務的成員）少於 3 個時回傳空清單。

`docs/compatibility.md` 把文字輸出列為不穩定（版面與措辭可在任何版本更動），`--json` 列為穩定（只加欄位）。

## Goals / Non-Goals

**Goals:**

- 看一行 backend 就能判斷它是 primary 還是 backup，以及它是否是「healthy 但最近一次檢查失敗」。
- 看 `peers:` 行就能知道群組現在投不了票，所有 reason 都是本機自己判的。
- 文件補齊現有標記與 `--json` 欄位的說明。

**Non-Goals:**

- 不改 `--json`、結束碼、daemon、檢查、投票、store 格式。
- 不加 slow start、`locality` site、peer 指標進文字輸出。
- 不刪 `voted`、`checks N/M`、`leaves out`。
- 不做每個服務各一行的投不了票提示。

## Decisions

### `backup` 標記放在 pool 欄之後、`local-up` 之前

標記是附加在行尾的字串，不是對齊的欄位：`pool` 是欄位（只有 `fastest` 的服務才有寬度），而 `local-up`、`voted`、`FLAPPING` 都是附加的片段。`backup` 屬於 backend 自身的靜態屬性，所以放在欄位之後、其他動態標記之前，這樣 backup 與 pool 這兩個「角色」標記相鄰。顏色不上色：它是常態，不是警訊。

替代方案：做成固定寬度的欄位。拒絕：多數服務沒有 backup，欄位會在大多數行留下空白，也要改所有範例的欄寬。

### `last check failed` 的條件是 state 為 `healthy` 且 `reason` 有值

不需要新欄位。`unhealthy` 與 `unknown` 的 backend 本來就帶 reason，不加標記；`unrecognized`（新版 daemon 寫的狀態）也不加，因為讀者無法判斷。標記放在 `FLAPPING` 之後、`reason` 之前，上黃色，與 `local-up`、`FLAPPING` 同一組警訊。

措辭只陳述事實（最近一次檢查失敗），不寫「excluded」：手冊第 5 章說，所有 healthy backend 都在失敗時它們全部仍是候選者，所以「已被排除」並非恆真。手冊會在標記處解釋它通常代表這台主機已不選它，並指出例外。

替代方案：用「excluded」「skipped」。拒絕：在全部失敗的情形會誤導。

### 「投不了票」只在整個群組都投不了時，用 `peers:` 行的後綴表示

後綴文字為 `, too few live members to vote`，接在 `not live` 部分之後。條件與 `voted` 共用同一個 `assigned_checkers` 結果：算得出（`listen` 與成員位址有效），至少有一個顯示的 backend 所屬的服務沒有被本機略過（`services_off` 不含它），而且這些 backend 都沒有 assigned checker。它與 `checks N/M backends here` 互斥（後者需要至少一個 voted backend，而這需要 assigned checker 存在）。

候選者剛好 3 個時仍有 assigned checker（三個全是本機加上兩個成員），所以不顯示；那是可以投票的狀態。

替代方案一：每個服務各一行（`peers: <service> has too few live members to vote`）。更精確，但在成員用 `peers = false` 略過不同服務的群組裡會一次出現很多行，而 `--json` 的 `assigned_checkers` 已能回答每個 backend 的問題。拒絕，列為 Non-Goal。

替代方案二：只在 `live` 數小於 2 時顯示。拒絕：略過服務的成員會讓實際候選者變少，用 `assigned_checkers` 的結果才與 daemon 一致。

### 不改 `checked_at` 的行為，只確認並記錄

`voted` 的 backend 行顯示 `<age> ago`，來自 `checked_at`。daemon 套用結果時（`schedule.rs`）以 `unix_ms()` 寫入 `checked_at`，投票得出的結果也是這樣套用，所以 age 反映的是「結果套用的時間」，不一定是本機實際連線的時間。**結論（apply 時讀 `schedule.rs` 的檢查迴圈與 `writer.rs` 確認）**：每個檢查時間點，不論是本機自己檢查，還是只由投票決定，迴圈都會建立 `BackendUpdate`，其 `checked_at` 是 `unix_ms()`；只有本機自己檢查時才有延遲樣本（`sample_us`），所以投票決定的時間點更新 `checked_at` 與 state/reason，但不更新 latency。結果是：`voted` 的 backend 行顯示的 `<age> ago` 是「結果套用的時間」，可能很新，而本機實際連線的時間更早（約每三個 interval 一次）。這可能讓人誤以為本機剛檢查過，所以中英手冊在 `voted` 的說明裡寫明。本變更不改 daemon 行為。

### 文字輸出變更不算不相容變更

依 `docs/compatibility.md`，文字輸出的版面與措辭可以在任何版本更動，所以不需要旗標或過渡期；CHANGELOG 仍寫明新增的標記與後綴，讓用 grep 解析它的人知道。

## Implementation Contract

**Behavior**

- 一個 `backup = true` 的 backend，其行含 `backup`（在 `pool` 之後、`local-up` 之前）。
- 一個 `state = healthy` 且 `reason` 有值的 backend，其行含 `last check failed`（在 `FLAPPING` 之後、reason 之前）；其他 state 不含。
- 當群組無法投票（見上）時，`peers:` 行以 `, too few live members to vote` 結尾（在 `not live ...` 之後）；其他情形的 `peers:` 行與現在相同。

**Interface / data shape**

- 文字輸出新增三個字串：`backup`、`last check failed`、`, too few live members to vote`。
- `--json`、結束碼、store、daemon：完全不變。

**Failure modes**

- `listen` 缺漏、無效或成員位址無效時，`assigned_checkers` 為 `None`，不顯示「投不了票」後綴（與 `voted` 一致，靜默）。
- daemon 不是 `running` 時沒有 `peers:` 行，所以也沒有後綴。
- 沒有 `backup` 欄位的舊 store（版本 1）讀為 `false`，不顯示標記。

**Acceptance criteria**

- `cargo test -p emfor-daemon --lib status::` 通過，包含新增的案例：backup、healthy 加 reason、unhealthy 加 reason 不加標記、投不了票（成員都不 live、`services_off` 使 candidates 不足）、可投票（剛好 3 個候選者）不顯示後綴、服務被本機略過不顯示後綴。
- `spectra validate emfor-status-clarity` 通過；delta spec 的逐字範例與測試一致（既有範例中受影響的列已更新）。
- 中英手冊的 `emfor status` 範例與標記清單、`docs/deploy/daemon.md` 的清單與 `--json` 欄位清單已補齊。
- 10 台、40 個 backend 的範例輸出，最長一行的寬度已量過並記在手冊或測試裡；`--watch` 的「Output fits the terminal」行為不變。

**Scope boundaries**

- 範圍內：`status.rs`、`status-command` spec、`docs/deploy/daemon.md`、中英手冊第 8 章（必要時第 5 章交叉說明）、CHANGELOG。
- 範圍外：daemon、檢查、投票、`--json`、結束碼、metrics、DNS、clients、store 格式、其他 spec。

## Risks / Trade-offs

- 有人用 grep 解析文字輸出（例如找 `ALL DOWN`）：新字串是附加的，不改既有字串，風險低；CHANGELOG 會寫明，並重申 `--json` 才是穩定介面。
- 行變長：`last check failed` 加上長 reason 會讓一行更寬。標記放在 reason 之前不改變 reason 的內容；`--watch` 已有截斷到終端寬度的行為。寬度以 10 台、40 個 backend 的範例實測，不憑估計。
- 「投不了票」只在全部可投票的服務都投不了時顯示：部分服務投不了的群組看不到提示。已在手冊說明並指向 `--json` 的 `assigned_checkers`。
