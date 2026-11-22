## Context

`emfor lookup --explain` 與 `--json` 的 `store` 物件由同一個 `Explanation`（`clients/rust/src/select.rs`）產生：client 模式由 `clients/rust/src/client.rs` 的 explain 填年齡與 `stale`，`--dns` 模式由 `crates/emfor-daemon/src/lookup.rs` 讀記憶體快照後自己填。文字輸出（`heartbeat_text`、`stale:` 說明）和 JSON（`store.heartbeat_age_ms`、`store.stale`）都在 daemon 的 `lookup.rs`。

`Explanation.heartbeat_age_ms` 是 `u64`，heartbeat 在未來時以 `saturating_sub` 夾成 0。上一個 change（emfor-future-heartbeat-stale）讓 `is_stale` 對未來超過 `max_staleness` 也回 true，於是 client 模式的 explain 會同時顯示「年齡 0」「stale」與「the heartbeat is older than 3.0s」。`emfor status` 在同一個 change 已改為帶符號年齡與 `in the future` 字樣；`--explain` 沒跟上。

## Goals / Non-Goals

**Goals:**

- client 模式 `--explain` 與 `--json` 在 heartbeat 比現在晚時，如實呈現（負的年齡、`in the future`、正確的 stale 說明），與 `emfor status` 的規則一致。
- 過去的 heartbeat 與已停止的 daemon（heartbeat 0）輸出不變。

**Non-Goals:**

- `--dns` 說明模式的 stale 規則：維持「`0` 或超過 3 秒的過去」才算 stale，未來視為新鮮（該模式本來就說明 DNS server 不看 heartbeat）。只有年齡的型別與呈現隨共用欄位變成帶符號。
- store 格式、daemon 寫入端、聯防訊息、新增 JSON 欄位。

## Decisions

### Explanation.heartbeat_age_ms 改為 i64，不另開欄位

沿用既有欄位，值域放寬為整數（未來為負數）。與 `emfor status --json` 的 `heartbeat_age_ms` 同一個做法，外部自動化只需處理一種慣例。替代方案：另開 `heartbeat_ahead_ms`（欄位多一個、兩個欄位要互相對照）；維持夾成 0 並只改文字（JSON 仍矛盾）。兩者都放棄。

Rust 公開 API 的 `Explanation` 是 `pub struct`，欄位型別改動是 API 變更；`emfor-client` 目前版本 0.7.x 且此欄位只在 `testing`/explain 用途出現，CHANGELOG 要寫明。

### 文字輸出依符號分兩種說法

年齡為負：`heartbeat <|age|> in the future`；`stale` 時的說明為 `stale: the heartbeat is more than <max> ahead of the clock`（過去維持 `stale: the heartbeat is older than <max>`）。年齡為正的輸出逐字不變，已停止的 daemon（age 超過十年）仍是 `no heartbeat (the daemon has stopped)`。

## Implementation Contract

**行為**

- client 模式，heartbeat = 現在 + 60 s、`EMFOR_MAX_STALENESS` 3 s：文字有 `heartbeat 1m00s in the future` 與 `stale: the heartbeat is more than 3.0s ahead of the clock`；`--json` 的 `store.heartbeat_age_ms` 為約 -60000（容許讀取時間造成的數十毫秒差），`store.stale` 為 true。
- heartbeat 在未來但不超過 `max_staleness`：`stale` false，文字 `heartbeat <age> in the future`，其餘同 fresh。
- heartbeat 過去或 0：輸出與改動前相同。
- `--dns A|AAAA`：stale 判斷不變；heartbeat 在未來時 `stale` false，年齡為負、文字為 `in the future`。

**介面**

- `Explanation.heartbeat_age_ms: i64`（`clients/rust/src/select.rs`），`Explanation` 預設值為 0。
- `crates/emfor-daemon/src/lookup.rs`：`heartbeat_text(age_ms: i64)`；`store.heartbeat_age_ms` 的 JSON 為整數（可為負）。

**失敗模式**

- 時間值超過 `i64` 範圍時以 `i64::MAX` 夾住，不 panic（與 `daemon_state` 相同）。

**驗收標準**

- `clients/rust/src/client.rs` 的單元測試：以 heartbeat 領先 60 s 的 store 呼叫 explain，`heartbeat_age_ms < -59_000`、`stale` true；領先 1 s 時 `stale` false 且年齡為負。
- `crates/emfor-daemon/tests/lookup.rs` 的整合測試：同一份未來 store，文字含 `in the future` 與 `ahead of the clock`，`--json` 的 `store.heartbeat_age_ms` 為負且 `store.stale` 為 true；現有 `is_u64()` 斷言（過去的 heartbeat）仍成立。
- 全部測試經 `tools/docker-test.sh` 在 Debian Docker 內通過。

**範圍**

- 範圍內：`Explanation` 型別、client 的 explain、`lookup.rs` 的文字與 JSON、lookup-command spec 的兩個 requirement、手冊第 12 章（中英）與 CHANGELOG。
- 範圍外：Non-Goals 列的各項。

## Risks / Trade-offs

- **`Explanation.heartbeat_age_ms` 型別改變。** 直接使用 `emfor_client::Explanation` 的 Rust 程式需要跟著改（`u64` → `i64`）。只有 explain 與測試用到，影響面小；CHANGELOG 寫明。
- **`--json` 出現負值。** 與 `emfor status --json` 相同的取捨，只在時鐘往回調時出現。
- **`--dns` 模式的年齡也變成帶符號**，而它的 stale 規則沒變：未來的 heartbeat 在 `--dns` 顯示 `in the future` 且 fresh。這與「DNS server 不看 heartbeat」的語意一致，不是矛盾，但手冊要避免誤導。
