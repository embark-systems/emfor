## 1. 資料格式與排除規則（emfor-format，先新增不改行為）

- [ ] 1.1 設計「加欄位，不加 `state` 值，不升 schema」，即需求「Drained backend record」：`BackendRecord` 新增選用欄位 `drain`（缺少讀作假，假時不序列化）與 `checked_state`（缺少讀作無，無時不序列化），`SCHEMA_VERSION` 仍為 2；驗證目標：`crates/emfor-format` 的單元測試——缺欄位的舊記錄可讀、沒有 drain 的記錄序列化結果與現在逐位元組相同（Backend without drain is unchanged）、drain 的記錄含 `"drain":true`，且既有的 schema 版本測試通過。
- [ ] 1.2 [after: 1.1] 需求「Drained backends are never selected」，設計「排除放在 `explain_tiers`，一處決定」：`explain_tiers` 在所有層級（含最後一層與心跳過期時）排除 `drain` 的 backend，新增 `Exclusion::Drained`（說明文字 `drained`），`refresh_all_down` 不把 drain 的 backend 算健康；驗證目標：規則表測試涵蓋 Drained backend is skipped、Not even the last tier、Everything drained（候選為空、層級為無），並且 `explain_tiers_matches_the_candidates_of_healthy_or_all` 把 drain 加進它列舉的組合仍成立。
- [ ] 1.3 [after: 1.2] 需求「Clients exclude drained backends」，設計「全部 backend 都已 drain」與 client 端：`emfor-client` 的 lookup 與 `explain` 用上述規則，全部 drain 時回 `ALL_DOWN`（`open` 也是），缺 `drain` 的記錄與現在相同；驗證目標：`clients/rust` 的測試涵蓋 Lookup with a drained backend、Lookup when all are drained、Older store，以及 `sticky` 的 key 移走又回來（Sticky key moves away）。

## 2. 設定與 daemon

- [ ] 2.1 需求「Draining a backend」，設計「只在本機，不進 digest，不給 peers」與「設定與 reload」：backend 表格的 `drain`（布林，預設假）加進 `config.rs` 的允許鍵與驗證，錯誤路徑 `...backends[n].drain`、訊息 `drain must be true or false`；`Backend` 新增 `drain`；全部 drain 的服務合法；`drain` 不進 `check_digest` 也不進 `check_changed`；驗證目標：設定單元測試 Drain value is validated、Drain does not change the digest，以及全部 drain 可通過驗證。
- [ ] 2.2 [after: 1.1, 2.1] 設計「真實狀態放在記憶體記錄，寫入 store 時才轉成對外的樣子」：`writer.rs` reload 時把 `drain` 從設定抄到記憶體記錄（比照 `backup`），`start_commit` 寫出時把 drain 的 backend 轉成 `state` unhealthy、`reason` `drained`、`drain` 真、`checked_state` 真實狀態，記憶體記錄不被修改；`fast_pool`（`state.rs`）排除 drain；驗證目標：writer 單元測試 Drained backend as stored、All backends drained（`all_down` 真）、轉換函式不改記憶體記錄、取消 drain 後寫回真實狀態且沒有歷史轉換與 slow start。
- [ ] 2.3 [after: 1.2, 2.2] daemon 的候選與 DNS：`select.rs` 與 `dns.rs` 經 `explain_tiers` 排除 drain，DNS 的 A、AAAA、SRV 不含它，全部 drain 時回 SERVFAIL，快組不含它；驗證目標：測試 Fast pool、Everything drained（DNS），以及每種 balance 方式（含 `sticky`、`failover`）在 drain 時都不挑它。
- [ ] 2.4 [after: 2.2] 整合測試：以 `drain = true` 啟動並 reload（tests/daemon.rs 風格），store 記錄內容、DNS 答案不含它、取消 drain 後恢復且檢查任務沒有重啟（Reload applies drain without resetting state）；驗證目標：該整合測試通過。

## 3. 看得見

- [ ] 3.1 [after: 2.2] 需求「Status shows drained backends」，`emfor status`：文字輸出標示 `drained` 與 `checks: <checked_state>`，`--json` 只在 drain 時新增 `drain` 與 `checked_state`，全部 drain 的服務依 `all_down` 走既有的結束碼規則；驗證目標：status 測試 Text output、JSON output，以及沒有 drain 時輸出與現在逐字相同。
- [ ] 3.2 [after: 1.2] 需求「Explain shows drained backends」，`emfor lookup --explain`（文字與 `--json`）顯示排除原因 `drained`，全部 drain 時無候選且結束碼 1；驗證目標：lookup 測試 Excluded with a reason、Everything drained。
- [ ] 3.3 [after: 2.2] 需求「Drain metric」，metrics：新增 `emfor_backend_drain{service,backend}`（每台 backend 輸出 0 或 1），`emfor_backend_state` 與 `emfor_backend_transitions_total` 照舊回報真實的檢查狀態、不新增標籤（`BackendCounters::STATES` 不改）；驗證目標：metrics 測試 Drained backend、Backend without drain。

## 4. 文件

- [ ] 4.1 [after: 3.1, 3.2, 3.3] 手冊第 3、5、8、10 章（中英）：`drain` 的設定與 reload、選擇規則、`status` 與 `lookup --explain` 與 metrics 的顯示、第 10 章把 HAProxy 的 `DRAIN` 與 `MAINT`、Consul 的維護模式對上並寫明 emfor 沒有 persistence 所以 drain 的 backend 一律不挑、舊 client 在最後一層的差異；驗證目標：`tools/check-manual-links.py` 通過，範例 TOML 用 `emfor check-config` 驗證過。
- [ ] 4.2 [after: 3.1, 3.2, 3.3] `docs/deploy/daemon.md`、`docs/compatibility.md`（新增兩個選用欄位與舊 client 的行為）、`examples/emfor.toml`（以註解示範 `drain`）、CHANGELOG 的 Unreleased；驗證目標：範例設定通過 `emfor check-config`，內容審閱。

## 5. 驗證

- [ ] 5.1 [after: 2.3, 2.4, 4.1, 4.2] Debian docker 閘門：`tools/docker-test.sh` 的 fmt、clippy `-D warnings`、`cargo test --workspace --all-features` 全過，現有的設定、writer、選擇、status、lookup、metrics 測試不改仍通過；macOS 與 FreeBSD 等發版前再測，標為未驗證。
