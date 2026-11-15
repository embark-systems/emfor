## Why

要讓一台 backend 暫時不接新流量（升級、重開機、調查），目前只能把它從設定檔拿掉再 reload：`weight` 沒有 0，也沒有維護狀態。拿掉之後 `emfor status` 看不到它，健康檢查也停了，加回來時得重新累計 `rise` 才知道能不能用，維運看不出「它是被拿掉」還是「它壞了」。

手冊第 10 章（從 HAProxy 或 Consul 搬過來）也把「把 backend 拿出來做維護」列為沒有對應的功能。

`/spectra-discuss`（2026-11-16）已經和使用者定下做法，並用程式核對過相容性：所有語言的 client 都包裝同一份 `libemfor`，讀到不認得的 `state` 一律當 `unhealthy`、不會失敗；但 state-store 規格寫明，語意不同於 `unhealthy` 的新 `state` 值必須提高 `meta/schema_version`，而 schema 升到 3 的 store 會讓所有舊 client 回 `INCOMPATIBLE`。所以這次不加新的 `state`，改在 backend 記錄加一個可忽略的欄位。

## What Changes

- 設定：backend 表格新增 `drain = true`（預設 `false`，寫法與 `backup`、`weight` 相同：`{ addr = "10.0.0.11:8080", drain = true }`）。只能用設定檔加 reload 來開關；reload 套用它時不重置 backend 的狀態。
- 選擇：已 drain 的 backend 不會被任何分配方式挑到，包含 `sticky` 與 `failover`，也不會出現在 `on_all_down = "open"` 的最後一層；DNS 的 A、AAAA 與 SRV 答案、`fastest` 的快組也都排除它。其餘 backend 照現有規則挑；沒有任何 backend 可挑時，`lookup` 回 `ALL_DOWN`、DNS 回 SERVFAIL，即使 `on_all_down` 是 `open`，因為沒有東西可以「照樣挑」。
- 檢查照跑：drain 期間仍檢查這台 backend，所以取消 drain 時馬上知道它能不能用，不用重新累計 `rise`。drain 本身不會啟動 slow start（它的真實狀態沒有轉換）；drain 期間才恢復健康的 backend，仍照平常從那次恢復起慢啟動。
- 存放：schema 不升版。backend 記錄新增選用欄位 `drain`（布林，缺少時讀作 `false`）。已 drain 的 backend 寫成 `state = "unhealthy"`、`reason = "drained"`、`drain = true`，並新增選用欄位 `checked_state`（只在 `drain` 為真時寫，是檢查得出的真實狀態）。舊 client 看到的是一台不健康的 backend；新 client 看到 `drain` 就在每一層都排除它。已知代價：舊 client 在「全部不健康且 `on_all_down = "open"`」的最後一層仍可能挑到它。
- 範圍：`all_down` 不把已 drain 的 backend 算作健康。
- 只在本機：drain 不傳給 peers、不進服務定義的 digest（`check_digest`），所以各台主機可以各自 drain；聯防群組對這台 backend 的投票照常進行。
- 看得見：`emfor status`（文字與 `--json`）顯示它已 drain 與真實的檢查狀態；`emfor lookup --explain` 為它顯示排除原因 `drained`；metrics 的 `emfor_backend_state` 繼續回報檢查得出的真實狀態，另加一個標示 drain 的指標，既有指標不改名、不改意義。
- 文件：手冊第 3、5、8、10 章（中英；第 10 章把 HAProxy 的 `DRAIN` 與 `MAINT`、Consul 的維護模式對上，並寫清楚 emfor 沒有 persistence 這件事所以不保留 sticky）、`docs/deploy/daemon.md`、`docs/compatibility.md`、`examples/emfor.toml`、CHANGELOG 的 Unreleased。
- 隨 0.5.0 之後的下一版發佈，這次不打 tag。

## Non-Goals

- 不做執行期的 drain（指令或訊號）：只有設定檔加 reload。
- 不把 drain 同步給 peers，也不讓聯防群組一起 drain。
- 不新增 `state` 值，不提高 schema 版本。
- 不改 `weight` 的範圍（仍是 1 到 100，沒有 0）。
- 不做 HAProxy `DRAIN` 那種保留既有 persistence 的模式：emfor 不轉送流量，已建立的連線本來就不受影響；問題只在新的查詢，而 drain 的 backend 對新查詢一律不挑。
- 不做「印出有效設定」（另一個項目）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: backend 表格新增 `drain` 鍵與其驗證；reload 套用 `drain` 不重置 backend 狀態；`drain` 不進 digest。
- `state-store`: backend 記錄新增選用欄位 `drain` 與 `checked_state`；已 drain 的 backend 的寫法；`all_down` 不把它算健康；不升 `schema_version` 的理由與舊讀者的行為。
- `backend-selection`: 已 drain 的 backend 從所有候選層排除（包含最後一層）、所有分配方式、DNS 的 A、AAAA、SRV 答案與 `fastest` 的快組排除；全部 backend 已 drain 時的結果。
- `client-lookup`: client 讀到 `drain` 時依上面的規則排除；缺少 `drain` 讀作 `false`；全部已 drain 時回 `ALL_DOWN`。
- `status-command`: 文字與 `--json` 顯示 drain 與真實的檢查狀態。
- `lookup-command`: `--explain` 為已 drain 的 backend 顯示排除原因 `drained`。
- `metrics-endpoint`: 新增標示 drain 的指標；`emfor_backend_state` 繼續回報真實的檢查狀態。

## Impact

- 程式：`crates/emfor-format/src/lib.rs`（`BackendRecord`、`refresh_all_down`）、`crates/emfor-format/src/rules.rs`（`explain_tiers`、`Exclusion`）、`clients/rust/src/select.rs`（client 的選擇與 `explain`）、`crates/emfor-daemon/src/config.rs`（backend 表格的 `drain`）、`crates/emfor-daemon/src/writer.rs`（reload 套用、寫入已 drain 的記錄）、`crates/emfor-daemon/src/state.rs`（`fast_pool` 排除）、`crates/emfor-daemon/src/select.rs` 與 `dns.rs`（候選）、`crates/emfor-daemon/src/status.rs`、`crates/emfor-daemon/src/lookup.rs`、`crates/emfor-daemon/src/metrics.rs` 與 `live.rs`。
- Spec：`docs/spectra/specs/` 下的 `service-config`、`state-store`、`backend-selection`、`client-lookup`、`status-command`、`lookup-command`、`metrics-endpoint`。
- 文件：`docs/manual/` 與 `docs/manual.zh_TW/` 的第 3、5、8、10 章、`docs/deploy/daemon.md`、`docs/compatibility.md`、`examples/emfor.toml`、`CHANGELOG.md`。
- 相容性：新增兩個選用欄位，屬 `docs/compatibility.md`「Store format」允許的相容變更；舊 client 讀到的是 `unhealthy`，只有最後一層的行為不同（見上）。不改 plugin 協定、不改 DNS 以外的對外介面。
