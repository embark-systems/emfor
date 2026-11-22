## Context

reload 在 `crates/emfor-daemon/src/cli.rs` 的 SIGHUP 分支：`load(&config_path, Some(&current), plugins)` 回傳新設定，成功後呼叫 `supervisor.reload(&new)`、`plugins.commit`、聯防 `reload`，最後記 `config reloaded: N services, M backends`。此時舊設定 `current` 與新設定 `new` 都在手上。`Supervisor::sync` 與聯防 `PeersHandle::reload` 都不記任何增減。`config::read_layers_with` 以 `read_trusted` 讀每個檔案（回傳 `String`，原樣的 UTF-8 位元組），`Layers.files` 列出讀到的檔案；daemon 的 `log_files_read` 只在設了 `include_dir` 時才有東西可記，而且沒有雜湊。drain、weight、site、backup 在 `Backend`，其餘 service 設定在 `Settings`，檢查定義摘要在 `ServiceConfig.digest`，`check_changed` 決定 reload 時 backend 狀態是否重來（`checker_id` 或 `check_options` 不同；只有 `timeout` 不同時摘要變、狀態不重來）。專案已有 `sha2` 相依。

## Goals / Non-Goals

**Goals:**

- 維運事後能從 log 查到每次 reload 改了什麼，以及當時每個設定檔的內容雜湊。
- 不洩漏機密：不記 `check_options` 的值，不記密鑰內容或其雜湊。
- 欄位名可 grep、可告警；差異很大時 log 不被灌爆。

**Non-Goals:**

- JSON log、SNMP、hook、另開稽核檔、合併設定的雜湊。
- DNS、metrics、store 的差異（要重啟，不在 reload 路徑）。
- 改變 reload 的套用行為或 `Supervisor::sync`。
- 記「誰」改的：daemon 收到的 SIGHUP 不帶寄件者。

## Decisions

### 差異在 cli.rs 以純函式算，不碰 Supervisor::sync

新模組 `crates/emfor-daemon/src/audit.rs` 提供 `changes(old: &Config, new: &Config) -> Vec<Change>`，`Change` 是 enum，每個變體一種稽核事件；另有 `log_changes(&[Change])` 負責 50 行上限與寫 tracing。reload 成功分支在 `supervisor.reload(&new)` 之後、`config reloaded` 那一行之前呼叫。純函式容易單元測試，套用邏輯一行不動。替代方案：在 `Supervisor::sync` 內記——要把 log 散在加入、移除、更新三處，且聯防另有一條路，放棄。

### 事件與固定欄位

每行 info，訊息文字不承諾穩定，`key=value` 欄位名穩定（只加不改）。共同欄位 `change`（事件種類）；其餘依種類：

| `change` | 其他欄位 |
| --- | --- |
| `service_added`、`service_removed` | `service`、`backends`（個數） |
| `backend_added`、`backend_removed` | `service`、`backend` |
| `backend_changed` | `service`、`backend`、`field`（`weight`／`site`／`backup`／`drain`）、`from`、`to` |
| `service_changed` | `service`、`field`（`check`／`interval`／`timeout`／`rise`／`fall`／`jitter`／`on_all_down`／`balance`／`peers`／`slow_start`／`dns_ttl`）、`from`、`to`；`check` 是檢查名稱（如 `tcp`），不含選項 |
| `check_definition_changed` | `service`、`digest_from`、`digest_to`、`resets_state`（`true`／`false`，即 `check_changed`） |
| `peer_member_added`、`peer_member_removed` | `peer`（位址） |
| `peers_changed` | `field`（`encrypt`／`accept_plaintext`／`secret_file`／`previous_secret_file`）、`from`、`to`、`applies`（`now`，或 `next_start` 給 `previous_secret_file`） |

`site` 沒設時值寫 `-`；`slow_start`、`timeout` 等時間用與設定檔相同的寫法。`secret_file` 與 `previous_secret_file` 記路徑，不是內容。`timeout` 變更同時會讓 `digest` 變，但 `resets_state=false`。

### 絕不記 check_options 的值

`check_options` 可能含密碼路徑以外的設定（例如 `expect` 字串、帳號）。檢查定義改變時只記摘要 digest 的舊與新（既有的 `check_digest`，前 16 位元組的 hex）和 `resets_state`，操作者可用 digest 對照「是否同一個定義」，不可從 log 還原選項。

### 設定檔雜湊用原始位元組的 SHA-256

`read_layers_with` 對每個成功讀到的檔案（主檔與片段，讀取順序）記下 `sha256(text.as_bytes())` 的小寫 hex，放進 `Layers`（新欄位，與 `files` 平行）。`read_trusted` 以 `read_to_string` 讀，所以位元組就是檔案內容，操作者用 `sha256sum <file>` 可核對。每個檔案一行 info：`config file read` 帶 `file`、`sha256` 欄位。啟動（成功）與每次 reload 都記，不論有沒有 `include_dir`——現在沒設時一行都沒有。被拒絕的 reload（驗證失敗，但檔案讀到了）也記，因為「哪一版內容被拒絕」正是稽核要的；讀取本身失敗時沒有可用的雜湊，不記。不做合併後設定表的雜湊，它在 emfor 外無法重現。

### 每次 reload 最多 50 行差異

前 50 個事件依序記，其餘合併成一行 `change=more`、`omitted=<N>`。排序：service 名稱字母序，service 內先 service 層、再 backend 層（backend 依 id），最後聯防。常數 `MAX_AUDIT_LINES = 50`。一次換掉 80 個 service 時 log 只增加約 51 行。

### 不記的東西與誰改的

沒有差異時不多記一行。daemon 記不到「誰」：SIGHUP 不帶寄件者。手冊要寫：誰改的靠 auditd 監看設定目錄（例：`auditctl -w /etc/emfor -p wa`）或設定管理工具的紀錄，emfor 的 log 提供「什麼、何時、哪一版內容」。

### 穩定性承諾

欄位名與 `change` 的值只加不改；整句文字不穩定。手冊第 15 章的「Not stable: log lines」加例外說明。

## Implementation Contract

**行為**

- reload 成功：對 `changes(current, new)` 的每個事件記一行 info（最多 50 行，其餘一行 `change=more omitted=N`）；`config reloaded` 照記。
- 啟動成功、reload 成功或被驗證拒絕：每個讀到的檔案一行 `config file read`，帶 `file` 與 `sha256`；沒設 `include_dir` 時也記主檔。
- 絕不出現在任何稽核行：`check_options` 的值、密鑰內容或其雜湊、`secret_file` 以外檔案的內容。

**介面**

- `audit::changes(&Config, &Config) -> Vec<Change>`；`audit::log_changes(&[Change])`。
- `config::Layers` 新增每個檔案的 SHA-256 hex（與 `files` 平行）；`cli.rs` 的 `ReadFiles` 帶著它，`log_files_read` 改記 `file`、`sha256`。

**失敗模式**

- 檔案讀不到：沿用既有的錯誤，不記雜湊。
- 雜湊只是 log 內容，算它不會讓 reload 失敗。

**驗收標準**

- `audit.rs` 單元測試涵蓋上面表格的每一種 `change`，含 `check_options` 的值不出現在輸出、`timeout` 單獨變更時 `resets_state=false`、`checker_id` 或選項變更時 `resets_state=true`、沒有差異時回空、超過 50 個事件時多出的合併成一行。
- 整合測試（`crates/emfor-daemon/tests/`）：daemon 啟動後 log 有 `config file read` 與正確的 `sha256`（與測試用 `sha256` 算的值相同，沒設 `include_dir`）；改設定加一個 backend 並 drain 另一個後 SIGHUP，log 有 `backend_added` 與 `backend_changed ... field=drain`，且不含 `check_options` 的值；被拒絕的 reload 仍記雜湊。
- 全部測試經 `tools/docker-test.sh` 在 Debian Docker 內通過。

**範圍**

- 範圍內：差異與雜湊的 log、50 行上限、手冊第 12、13、15 章（中英）、CHANGELOG、service-config spec。
- 範圍外：Non-Goals 列的各項。

## Risks / Trade-offs

- **欄位名一旦承諾就難改**：所以表格只放有把握的欄位，之後只能加。
- **log 輪替或遺失**：稽核仰賴 journal／log 收集；手冊提醒把 log 送到集中的紀錄系統。
- **雜湊的是讀到的文字**：若檔案在讀與驗證之間被改，log 的雜湊是 daemon 實際讀到的那一版，這正是想要的。
