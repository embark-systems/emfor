## Why

服務設成 `on_all_down = "open"` 而沒有任何健康的 backend 時，client 與 DNS server 會把**所有** backend 都當候選，`unknown` 和 `unhealthy` 一視同仁。但兩者的意義不同：

- `unhealthy` 是已經連續失敗 `fall` 次、確定壞掉的 backend。
- `unknown` 是還沒有定論的 backend，包括：
  - daemon 剛啟動、新增或 reload 後重新開始檢查，還沒達到 `rise` 或 `fall`；
  - 檢查一直回 `error` 超過 `fall × interval`，退回 `unknown`。

例如一台 `unhealthy`、一台剛加入的 `unknown`，`failover` 服務目前會回傳 `unhealthy` 那台，因為它排在前面。`open` 的用意是「與其失敗，不如試試看」，試還沒確定壞掉的 backend，比試確定壞掉的更有機會成功。

## What Changes

- **行為改變**：store 新鮮、`on_all_down = "open"`、沒有任何健康的 backend 時，候選改成分三層：
  1. 有最近一次檢查通過的 `unknown` backend（`state` 為 `unknown` 且 `reason` 為 `null`）時，候選只有這些；
  2. 否則有任何 `unknown` backend 時，候選為全部 `unknown` backend（最近一次檢查失敗的也算）；
  3. 否則候選為全部 backend，和現在一樣。
- 規則放在 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all`。這個函式由 daemon 和 client 共用，所以以下都跟著改：
  - client 查詢：Rust API、C library，以及 Go、Python、Perl、PHP wrapper；
  - DNS server 的 A、AAAA 回答與 SRV 回答。
- 各 `balance` 方法的挑選方式不變，只是作用在新的候選上。
- 既有的 conformance 案例「open service with no healthy backend picks from all」原本預期回傳 `unhealthy` 的 `10.0.1.5:5432`，改為預期 `unknown` 的 `10.0.1.6:5432`。另外新增案例涵蓋這三層。
- 更新手冊說明 `open` 的段落（第 1、5、7 章，英文與中文）、`docs/deploy/dns-resolver.md` 與 `CHANGELOG.md`。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `client-lookup`：requirement「Candidates follow health and on_all_down」在 `open` 且沒有健康 backend 時，改為 `unknown` 優先的三層。
- `backend-selection`：requirement「Candidate backends per address family」做同樣的改變。
- `dns-server`：requirement「SRV answers」的候選做同樣的改變。

## Impact

- 受影響的 spec：client-lookup、backend-selection、dns-server
- 受影響的程式碼：
  - crates/emfor-format/src/rules.rs：`healthy_or_all` 加上 `unknown` 優先的兩層，以及 `healthy_or_all_table` 單元測試
  - crates/emfor-daemon/src/select.rs：A/AAAA 的 `candidate_sets_example` 與 SRV 的候選測試
  - clients/rust/src/select.rs：`candidates_follow_health_and_on_all_down` 測試
  - tests/conformance/v1/cases.json：修改一個既有案例、新增案例；所有 client 介面都會跑到
  - docs/manual/01-concepts.md、docs/manual/05-backend-selection.md、docs/manual/07-dns.md 與對應的 docs/manual.zh_TW 章節、docs/deploy/dns-resolver.md、CHANGELOG.md
- 升級期間：舊版 client 仍從全部 backend 中挑，新版 client 與 daemon 的 DNS 回答則先用 `unknown`。兩者都仍是合法的 `open` 行為。
