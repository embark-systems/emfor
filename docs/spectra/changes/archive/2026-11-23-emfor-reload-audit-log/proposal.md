## Why

封閉環境（電信機房）事後要查得到「設定什麼時候改了什麼」。現在 reload 成功只記 `config reloaded: N services, M backends`：service 或 backend 增減、`drain` 開關、聯防 members 與密鑰設定的變更都沒有任何 log，設定檔的內容也沒有雜湊可以和變更管理紀錄對照；`config file read: <path>` 只在設了 `include_dir` 時才出現。TODO 73 第 4 項（2026-11-22 封閉環境適用性評估）列為「小到中」。

## What Changes

- reload 成功時，把舊設定與新設定的差異逐項記成 info 行（`key=value` 欄位）：service 增減、backend 增減、backend 的 `weight`／`site`／`backup`／`drain` 變更、service 的設定欄位變更、檢查定義變更（只記摘要 digest 的舊與新，並說明該 service 的 backend 是否從 `unknown` 重來）、聯防的 members 增減、`encrypt`／`accept_plaintext` 切換、`secret_file` 路徑變更、`previous_secret_file` 變更（記「下次啟動才生效」）。
- 每次 reload 最多記 50 行差異，其餘以一行「and N more」帶 `omitted` 欄位交代。
- 每個讀到的設定檔（主檔與 `include_dir` 片段）記 SHA-256（原始位元組、小寫 hex），啟動與每次 reload 都記，不論有沒有設 `include_dir`；被拒絕的 reload 只要檔案讀到了也記。
- 絕不記 `check_options` 的值，也不記密鑰內容或其雜湊。
- 欄位名穩定（只加不改），整句文字不穩定；手冊第 15 章、第 12 章說明，並寫明 daemon 只能記「什麼、何時」，記不到「誰」（SIGHUP 不帶寄件者），誰改的靠 auditd 監看設定目錄或設定管理工具的紀錄。

## Non-Goals

- JSON log、SNMP、狀態變更 hook（TODO 73 第 3 項）。
- 另開稽核檔（會帶來權限、輪替、磁碟滿的問題）。
- 合併後設定表的雜湊（在 emfor 外無法重現）。
- plugin 啟停的 log（已有）；DNS、metrics、store 的設定差異（這些要重啟，不在 reload 路徑）。
- 改 `Supervisor::sync` 或 reload 的套用行為。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: 新增 requirement「Configuration audit log」：reload 差異行、設定檔雜湊、50 行上限、不記的內容。

## Impact

- `crates/emfor-daemon/src/cli.rs`：reload 成功後呼叫差異記錄；啟動與 reload 記設定檔雜湊。
- `crates/emfor-daemon/src/config.rs`：`Layers` 記每個讀到的檔案的 SHA-256。
- 新模組 `crates/emfor-daemon/src/audit.rs`：以純函式算兩份 `Config` 的差異並記 log。
- 文件：docs/manual 與 docs/manual.zh_TW 的第 12、13、15 章，CHANGELOG.md。
