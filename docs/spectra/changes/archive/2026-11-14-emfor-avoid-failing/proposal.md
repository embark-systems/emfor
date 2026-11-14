## Why

backend 從健康轉為不健康之前，要連續失敗 `fall` 次。在這段期間它仍是 `healthy`，client 與 DNS server 照樣把它當候選，應用程式每次挑到它都可能連線失敗。以 `interval = 2s`、`fall = 3` 為例，一台剛掛掉的 backend 最多還會被挑中約 6 秒。store 其實已經記錄了這件事：每個 backend 的 `reason` 在最近一次結果是 `down` 或 `error` 時有值，`up` 之後是 `null`。但目前的選擇規則只看 `state`，沒有用到 `reason`。

## What Changes

- 新增「正在失敗」的定義：`state` 為 `healthy`、但 `reason` 不是 `null` 的 backend；`state` 為 `healthy` 且 `reason` 為 `null` 的 backend 稱為「通過」。
- 候選規則多一層。store 新鮮時：
  1. 有任何通過的 backend，候選就只有通過的 backend。
  2. 沒有通過的、但有健康的 backend，候選為全部健康的 backend（正在失敗的也算，和現在一樣）。
  3. 沒有任何健康的 backend，由 `on_all_down` 決定，和現在一樣。
- 這條規則在 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all`，由 daemon 和 client 共用，所以同時影響：
  - client 查詢（Rust API、C library 以及 Go、Python、Perl、PHP wrapper）；
  - DNS server 的 A、AAAA 回答與 SRV 回答。
- store 過期時不看 `reason`：沒有任何 backend 算健康，行為不變。
- 各 `balance` 方法的挑選方式不變，只是作用在新的候選上：
  - `locality` 與 `fastest` 的偏好，在新的候選中套用；
  - `sticky` 的 rendezvous hashing 只在新的候選中計算。
- `tests/conformance/v1/cases.json` 新增測試案例，涵蓋這三層。
- 更新手冊 `docs/manual/05-backend-selection.md` 與 `docs/manual.zh_TW/05-backend-selection.md`。

## Non-Goals (optional)

- 不改 store 格式，也不新增欄位。`reason` 已在格式版本 1 中，舊版 daemon 寫的 store 一樣適用。
- 不區分 `down` 與 `error`：store 只記錄訊息，不記錄結果種類，兩者都算正在失敗。
- 不改 SRV 的 priority 編法：正在失敗的 backend 在有通過的 backend 時不出現在 SRV 回答中，而不是改給較低的優先順序。
- 不新增 client API（例如回報連線失敗），也不新增設定選項來關閉這個行為。
- 不改 `emfor status`、`/metrics`、`all_down` 欄位或 `on_all_down` 的語意。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `client-lookup`：requirement「Candidates follow health and on_all_down」加上「通過的 backend 優先」這一層。
- `backend-selection`：requirement「Candidate backends per address family」加上同樣的一層。
- `dns-server`：requirement「SRV answers」的候選也加上同樣的一層。

## Impact

- 受影響的 spec：client-lookup、backend-selection、dns-server
- 受影響的程式碼：
  - crates/emfor-format/src/rules.rs：`healthy_or_all` 加上通過優先的一層，以及單元測試
  - crates/emfor-daemon/src/select.rs：A/AAAA 與 SRV 的候選測試
  - clients/rust/src/select.rs：`candidates` 的測試
  - tests/conformance/v1/cases.json：新增案例；所有 client 介面都會跑到
  - docs/manual/05-backend-selection.md、docs/manual.zh_TW/05-backend-selection.md
  - CHANGELOG.md
