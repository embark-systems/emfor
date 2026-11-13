## Context

crates/emfor-daemon/src/status.rs 的 `peer_lines(state, snap, checkers)` 產生所有 `peers:` 行：先是 `peers: N/M live`（含 `not live` 與 `checks n/m backends here`），再依顯示中服務的名稱順序產生「check definition of <服務> differs at <成員>」。`PeerMember` 已有 `services_off`（`#[serde(default)]`，emfor-peers-off-announce 加入），`snap.services` 是 `--service` 篩選後、依名稱排序的顯示中服務。`peer_lines` 只在 daemon 執行中、`meta/peers` 形狀正確時被呼叫。

## Goals / Non-Goals

**Goals:**

- 文字輸出直接看得到哪個成員不參加哪些服務，與 `--json` 的資料一致。

**Non-Goals:**

- 不在文字輸出另列本機自己的 `services_off`（本機不參加的服務已由沒有 `voted` 標記與 `checks` 數字反映，且在設定檔裡看得到）。
- 不改 `--json`、結束碼、watch 模式的固定行數（store 與 daemon 兩行），也不新增指標。
- 不在 backend 行加新標記。

## Decisions

### 成員不參加的服務放在獨立的 peers 行

每個相關成員一行 `peers: <成員> leaves out <服務...>`，放在 `peers: N/M live` 之後、check definition 行之前。替代方案是把名單接在 `peers: N/M live` 那一行後面——成員多時該行會過長，而且與「check definition differs」一成員一服務的分行風格不一致。成員依 `meta/peers` 的順序（設定檔順序），服務依名稱排序，與其他行的排序規則相同。

### 只列顯示中的服務

只列 `snap.services`（`--service` 篩選後）裡的服務，與「check definition differs」只列顯示中服務的規則一致；成員的清單裡沒有任何顯示中服務時不印那一行。

## Implementation Contract

**行為：** daemon 執行中、`meta/peers` 形狀正確時，`emfor status` 在 `peers: N/M live` 之後，為 `services_off` 含顯示中服務的每個成員印出 `peers: <成員> leaves out <服務 1> <服務 2> ...`，接著才是 check definition 行。daemon 停止或過期、`meta/peers` 缺少或不是 JSON 物件時不印。

**介面：** 只改 `peer_lines` 的輸出；`PeerState`、`PeerMember`、`--json` 輸出與結束碼不變。

**失敗模式：** 成員缺 `services_off` 時視為空（既有的 `#[serde(default)]`），不印該成員的行；不會因此報錯。

**驗收：** status.rs 單元測試重現 status-command 情境「Members that leave services out」的完整行序，以及範例「leaves-out lines for member 10.0.0.6:5302」五列；既有 `peers:` 相關測試不改仍通過；暫時拿掉新行時新測試失敗。macOS 與 Debian 映像的 fmt、clippy、test 通過。

**範圍：** 範圍內為 `peer_lines`、其單元測試、08-operations（en、zh_TW）與 CHANGELOG。範圍外見 Non-Goals。

## Risks / Trade-offs

- [成員很多、不參加的服務很多時輸出變長] → 只列顯示中的服務，用 `--service` 可縮小；每個成員最多一行。
