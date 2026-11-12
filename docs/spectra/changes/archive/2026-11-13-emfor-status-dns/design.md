## Context

emfor-status-metrics 讓 daemon 在啟動交易中寫入 `meta/metrics_listen`，位址經 `writer::spawn_live` 與 `spawn_inner` 的參數 `metrics_listen: Option<Vec<String>>` 帶入，再由 `cli::metrics_listen(&Config)` 轉成字串；`emfor status` 讀出後在 daemon 行加上 `, metrics <位址>`。`[dns] listen` 和 `[metrics] listen` 一樣不能 reload。

## Goals / Non-Goals

**Goals:**

- `emfor status` 的 daemon 行與 `--json` 顯示 DNS 實際在聽的位址，來源與 metrics 位址相同（store，由執行中的 daemon 寫入）。
- 兩個 listen 鍵共用同一套寫入、讀取與顯示程式，不再各寫一份。

**Non-Goals:**

- 不顯示 `[dns] zone`（可 reload，需另行處理 reload 時的更新）。
- 不改 client、DNS 伺服器與 `meta/schema_version`。

## Decisions

### 位址比照 meta/metrics_listen 寫進 store

`meta/dns_listen` 的格式、寫入時機與刪除規則都和 `meta/metrics_listen` 相同。

- 理由：同一類資訊用同一種做法，status 的讀取與顯示可以共用；store 中的值才是執行中 daemon 實際綁定的位址。
- 替代方案：status 讀設定檔的 `[dns]` —— 與 emfor-status-metrics 捨棄的理由相同，捨棄。

### writer 以 ListenKeys 結構帶入兩個位址

`writer::spawn_live` 與 `spawn_inner` 的最後一個參數由 `metrics_listen: Option<Vec<String>>` 改為 `listen: ListenKeys`，`pub struct ListenKeys { pub dns: Option<Vec<String>>, pub metrics: Option<Vec<String>> }`（`Default` 為兩者皆 `None`）。writer 在 fresh start 時對兩個鍵各自 `Op::Put` 或 `Op::Delete`。

- 理由：再加一個 `Option<Vec<String>>` 參數，兩個同型別參數容易傳反；結構的欄位名稱讓呼叫端不會弄錯。
- 替代方案：再加一個 `dns_listen` 參數 —— 同型別位置參數易錯，捨棄。

### daemon 行順序：site、dns、metrics

`, dns <位址>` 放在 `site` 之後、`metrics` 之前，與設定檔中 `[dns]` 在 `[metrics]` 之前的順序一致。

## Implementation Contract

- **常數**：`crates/emfor-format/src/lib.rs` 新增 `pub const KEY_DNS_LISTEN: &str = "meta/dns_listen";`。
- **寫入**：`writer::ListenKeys`；`spawn_live(sink, rx, live, history, listen: ListenKeys)`；`spawn_with_tick` 傳 `ListenKeys::default()`。`cli::metrics_listen(&Config)` 改為 `cli::listen_keys(&Config) -> ListenKeys`，`dns` 取自 `config.dns`、`metrics` 取自 `config.metrics`，位址以 `SocketAddr` 的 `Display` 轉成字串。fresh start 寫入或刪除兩個鍵，reload 不動。
- **讀取與顯示**：`status::Snapshot` 新增 `dns_listen: Option<Vec<String>>`，Reader 用既有的 `metrics_listen` 解析函式（改名為 `listen_value`）讀 `KEY_DNS_LISTEN`；`header` 在 running 時依 site、dns、metrics 的順序輸出；`--json` 新增 `dns_listen`。
- **失敗模式**：值無法解析為字串陣列時不顯示、JSON 為 `null`，不影響結束碼。
- **驗收**：
  - writer 單元測試：`dns` 為 `Some` 時 fresh start 後 `meta/dns_listen` 為 `["127.0.0.1:5300","[::1]:5300"]`、reload 不改、下一次以 `None` 啟動後鍵不存在；既有 `metrics_listen_written_at_start` 改用 `ListenKeys` 後照樣通過。
  - cli 單元測試：`listen_keys` 對含 `[dns]` 與 `[metrics]`（含 IPv6）的設定回傳對應字串，兩表都沒有時兩者皆 `None`。
  - status 單元測試：spec「DNS part of the daemon line」表格五列、「DNS and metrics addresses on the daemon line」整行內容、「JSON DNS addresses」有值與 `null`；既有 metrics 測試照樣通過。
  - 整合測試：`crates/emfor-daemon/tests/daemon.rs` 的 `status_shows_metrics_addresses` 擴充為同時開 `[dns]`，daemon 行以 `, dns 127.0.0.1:<dns 埠>, metrics 127.0.0.1:<metrics 埠>` 結尾，`--json` 的 `dns_listen` 為該位址。
  - macOS 與 Debian 映像 gate 全數通過。
- **範圍內**：上述常數、writer 參數結構、cli 轉換、status 讀取與顯示、`docs/deploy/daemon.md` 的範例與欄位說明、`README.md` 與 `README.zh-TW.md` 快速上手的 status 範例行。
- **範圍外**：zone、client、DNS 伺服器行為、`--watch` 的變更列表。

## Risks / Trade-offs

- [舊版 daemon 寫的 store 沒有 `meta/dns_listen`] → 不顯示，JSON 為 `null`。
- [`spawn_live` 參數型別改變] → 只有 `cli::run` 與 writer 自己的測試呼叫，一起修改。
