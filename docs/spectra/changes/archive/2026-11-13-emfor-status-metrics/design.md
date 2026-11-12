## Context

`emfor status` 只讀設定檔的 `[store]` 表，其他資訊（心跳、啟動時間、`site`）都從 store 的 `meta/*` 鍵讀出。`meta/site` 由 writer 在處理 `SetLayout` 時寫入或刪除。`[metrics] listen` 在 emfor-metrics 中定為不能 reload，daemon 啟動時在 `cli::run` 綁定。

## Goals / Non-Goals

**Goals:**

- daemon 執行中時，`emfor status` 的 daemon 行與 `--json` 顯示 metrics 實際在聽的位址。
- 設定拿掉 `[metrics]` 後重啟，不留上一次的位址。

**Non-Goals:**

- 不顯示 `[dns] listen`。
- 不從設定檔讀 `[metrics]`。
- 不改 client、DNS 與 `meta/schema_version`。

## Decisions

### 位址從 store 讀，比照 meta/site

daemon 在啟動交易中寫入 `meta/metrics_listen`，status 從 store 讀。

- 理由：設定檔可能改了 `listen` 卻還沒重啟；store 中的值是正在執行的 daemon 寫的，和 `site` 一樣可信。status 也不必解析 `[store]` 以外的設定。
- 替代方案：status 讀設定檔的 `[metrics]`（`config::parse_metrics_listen`）—— 可能顯示尚未生效的位址，捨棄。

### 只在啟動交易寫入，由 spawn_live 帶入

`writer::spawn_live` 與 `spawn_inner` 多一個參數 `metrics_listen: Option<Vec<String>>`，writer 在 `set_layout` 的 `fresh_start` 分支中與 `meta/started_at` 一起 `Op::Put`（`Some`）或 `Op::Delete`（`None`）。reload 的 `SetLayout` 不碰此鍵。

- 理由：listen 不能 reload，值在整個執行期間固定；放在 spawn 參數只改 3 處 `spawn_live` 呼叫與 `spawn_inner` 的少數呼叫。
- 替代方案：加進 `WriterMsg::SetLayout` —— 原始碼中有 29 處 `SetLayout {` 字面量要改，而且每次 reload 都得帶一個不會變的值，捨棄。

### 值為 JSON 字串陣列，顯示以空白分隔

存成 `["127.0.0.1:5301","[::1]:5301"]`（`serde_json::to_string(&Vec<String>)`，位址以 `SocketAddr` 的 `Display` 產生，IPv6 自帶方括號）；status 顯示為 `metrics 127.0.0.1:5301 [::1]:5301`。

- 理由：JSON 陣列不會和位址內的字元混淆，與 store 其他 JSON 值一致；daemon 行本身用逗號分隔各項，位址之間用空白才不會被誤讀成下一項。
- 替代方案：逗號分隔的純字串 —— 與 daemon 行的逗號分隔衝突，捨棄。

## Implementation Contract

- **常數**：`crates/emfor-format/src/lib.rs` 新增 `pub const KEY_METRICS_LISTEN: &str = "meta/metrics_listen";`。
- **寫入**：`writer::spawn_live(sink, rx, live, history, metrics_listen: Option<Vec<String>>)`；`cli::run` 傳入 `config.metrics.as_ref().map(|m| m.listen.iter().map(ToString::to_string).collect())`。`spawn_with_tick` 傳 `None`。fresh start 時寫入或刪除，reload 不動。
- **讀取**：`status::Snapshot` 新增 `metrics_listen: Option<Vec<String>>`；Reader 讀 `KEY_METRICS_LISTEN`，`serde_json::from_str::<Vec<String>>` 失敗視為 `None`（只供顯示，不影響結束碼）。
- **顯示**：`header` 在 `Daemon::Running` 時，於 site 之後加 `, metrics ` 加上以空白連接的位址；`--json` 輸出 `metrics_listen`（陣列或 `null`）。
- **失敗模式**：值無法解析時安靜地不顯示；寫入失敗沿用現有 commit 失敗處理（記錄錯誤並計入 `emfor_store_commit_failures_total`）。
- **驗收**：
  - writer 單元測試：`Some` 時 fresh start 後鍵值為 `["127.0.0.1:5301"]`；先寫入再以 `None` fresh start 後鍵不存在；reload 的 `SetLayout` 不改鍵。
  - status 單元測試：spec「metrics part of the daemon line」表格五列、`--json` 的 `metrics_listen` 有值與 `null`。
  - 整合測試（`crates/emfor-daemon/tests/daemon.rs`）：以空埠開啟 `[metrics]` 啟動 daemon，`emfor status` 的 daemon 行含 `, metrics 127.0.0.1:<port>`；`--json` 的 `metrics_listen` 為該位址。
  - macOS 與 Debian 映像 gate 全數通過。
- **範圍內**：上述常數、writer、cli 呼叫、status 讀取與顯示、`docs/deploy/daemon.md` 的範例與 `--json` 欄位表。
- **範圍外**：DNS 位址、client、`--watch` 的變更列表（daemon 行本來就會重畫，不另外標記）。

## Risks / Trade-offs

- [舊版 daemon 寫的 store 沒有此鍵] → status 視為不存在，不顯示；JSON 為 `null`。
- [舊版 status 讀新版 store] → 不認識的鍵本來就忽略，無影響。
