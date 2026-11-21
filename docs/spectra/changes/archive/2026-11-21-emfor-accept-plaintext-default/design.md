## Context

`[peers] encrypt` 預設 `true`（0.4.0 起）：預設設定的成員送 `EMP2` 或 `EMP3`，只有明確設 `encrypt = false` 的成員送 `EMP1`。接收端的 `accept_plaintext` 卻預設 `true`，所以預設設定的成員仍接受 `EMP1`（`crates/emfor-daemon/src/peers/group.rs` 的 `check` 只在 `accept_plaintext` 為 `false` 時回 `Reject::Plaintext`），只記一行 `warn`。設定讀入時 `accept_plaintext` 與 `encrypt` 各自獨立取預設值，並有一條驗證：`accept_plaintext = false` 而 `encrypt = false` 是錯誤，因為這樣的 daemon 會送出自己拒收的訊息（`crates/emfor-daemon/src/config.rs`）。

## Goals / Non-Goals

**Goals:**

- 預設設定的群組不再接受 `EMP1`，加密的保護不再被預設值抵銷。
- 沒有任何現在合法的 `[peers]` 設定因這個變更變成不合法。
- 升級順序寫成文件：有成員還送明文時怎麼做。

**Non-Goals:**

- 不改 `encrypt` 的預設值、發送端的格式選擇、驗證規則或接收端的處理（`Group::check`）。
- 不改 `[store]` 的權限預設值（另一個 change）。

## Decisions

### accept_plaintext 的預設值跟著 encrypt 走

預設值在 `[peers]` 讀入時決定：`encrypt` 為 `true` 時 `accept_plaintext` 預設 `false`；`encrypt = false` 時預設 `true`；明確寫的值優先。

替代做法：無條件把 `accept_plaintext` 預設改成 `false`。這會讓每一個寫了 `encrypt = false` 卻沒寫 `accept_plaintext` 的設定落入現有的驗證錯誤（`accept_plaintext = false needs encrypt = true`），daemon 啟動失敗，也就是把「不相容」直接做成設定錯誤。跟著 `encrypt` 走不需要動那條驗證，`encrypt = false` 的使用者也不必改設定。代價是預設值要在讀入時多一個分支，手冊要寫兩種情況。

### 驗證與接收端處理維持不變

預設值只在讀入設定檔的那一處決定，`Group::check` 與 `set_accept_plaintext` 照舊收一個布林值。reload 時 `[peers] accept_plaintext` 與 `encrypt` 本來就會套用，預設值跟著新的 `encrypt` 重算，不需要另外處理。

## Implementation Contract

- **行為**：`[peers]` 沒有寫 `accept_plaintext` 時，`encrypt` 為 `true`（含沒寫 `encrypt`）→ daemon 丟棄每一個 `EMP1` 資料報，不檢查標籤，計入 `emfor_peer_messages_rejected_total{reason="plaintext"}`；`encrypt = false` → 接受 `EMP1`。明確的 `accept_plaintext` 不受影響。
- **設定錯誤**：不變。`encrypt = false` 與 `accept_plaintext = false` 同時出現仍是 `peers.accept_plaintext` 的錯誤，訊息文字不變。
- **失敗模式**：群組內有成員送 `EMP1`（`encrypt = false` 的成員，或 0.3.0），預設設定的接收端丟棄它的訊息，該成員在接收端看起來不存活，直到該成員改送加密訊息，或接收端明確設 `accept_plaintext = true`。
- **驗收**：`config.rs` 中涵蓋預設值的單元測試改成檢查 `encrypt` 沒寫、`true`、`false` 與明確的 `accept_plaintext` 各種組合；`tests/peers.rs` 新增預設設定的成員丟棄 `encrypt = false` 成員的 `EMP1` 的整合測試，`mixed_formats_during_an_upgrade` 的接收端補上 `accept_plaintext = true` 後仍通過；以 `tools/docker-test.sh` 跑 `cargo test --workspace --all-features`、fmt 與 clippy。
- **範圍內**：`config.rs` 預設值與其測試、`tests/peers.rs`、手冊（英文與繁體中文）、`docs/deploy/daemon.md`、`examples/emfor.toml` 註解、CHANGELOG。
- **範圍外**：`group.rs`、`task.rs` 的接收與重新設定邏輯、發送格式選擇、`[store]`。

## Risks / Trade-offs

- [群組在升級中一時看不到送明文的成員] → 文件寫明順序：先在每個還要收明文的接收端明確設 `accept_plaintext = true` 並 reload，等所有成員都送加密訊息，再移除。
- [升級中的 0.3.0 成員（只送、只收 `EMP1`）與新預設的成員互相看不到] → 0.3.0 成員本來就收不到加密訊息，既有的升級流程（在升級的主機設 `encrypt = false`）仍然成立：`encrypt = false` 的主機預設接受明文。
- [預設值依賴另一個鍵，讀手冊的人容易漏看] → `accept_plaintext` 與 `encrypt` 兩列都寫出兩種情況，範例表列出所有組合。

## Migration Plan

給還有成員送明文的群組（`encrypt = false` 的成員、0.3.0 成員），在發佈 0.7.0 後依序：

1. 把一台主機升級到 0.7.0 時，如果群組裡還有成員送明文，同一次設定變更為這台主機的 `[peers]` 加上明確的 `accept_plaintext = true`。`accept_plaintext` 這個鍵是 0.5.0 加入的，0.4.0 與 0.3.0 不認識，所以不能在升級之前先寫進舊版本的設定。已經是 0.5.0 或 0.6.0 的主機，可以在升級之前先加這一行並 reload。
2. 每台送明文的主機移除 `encrypt = false`（或設 `true`）並 reload。
3. 每台主機的日誌不再出現 `sends plaintext messages`、也沒有 `reason="plaintext"` 的丟棄之後，移除 `accept_plaintext = true` 並 reload。

回復：把 `accept_plaintext = true` 寫回設定並 reload，即回到 0.6 的行為。
