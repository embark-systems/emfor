## 1. 量測與狀態（group.rs）

- [x] 1.1 在 `crates/emfor-daemon/src/peers/group.rs` 讓 `check` 的錯誤帶偏差：新的錯誤型別（例如 `Rejected { reason: Reject, offset_ms: Option<i64> }`，並提供 `From<Reject>` 讓既有的回傳點維持一行），在通過 `site` 檢查之後、`stale` 判斷之前算出 `offset_ms = msg.ts - now_ms`；`Stale` 與 `Replay` 的錯誤都帶偏差，其他原因（格式、驗證、寄件者、`site`）不帶。`accept` 在成功與失敗兩條路徑都把偏差寫進該成員的 `PeerState`（新的 `clock` 欄位：最近三則偏差與最近一則的時間），`accept` 的對外簽章與既有的計數、`warn_rejected` 行為不變。驗證：group.rs 單元測試涵蓋 peer-group「Clock offset of members」的 `Offset beyond the limit is still measured` 與 `Offsets do not change the votes`（`stale` 訊息仍被計數、成員仍不存活、偏差有記下），每個測試標註 `// spectra-scenario: peer-group / <scenario>`；`tools/docker-test.sh 'cargo test -p emfor-daemon peers::group'` 通過。
- [x] 1.2 [after: 1.1] 依 design「量測的壽命」「門檻、連續與遲滯」「狀態與 log 放在 `Group`，每秒掃一次」，新增 `Group::update_clock(now: Instant)`，由每秒的 peer tick 在 `check_liveness` 之後呼叫：丟掉超過 10 秒（`CLOCK_KEEP`）的量測；依「最近連續三則偏差的絕對值都 ≥ 2000 毫秒且同號」進入、「最近一則 < 1500 毫秒或量測過期」離開，其間維持原狀態；進入時記一次 `warn`（成員、`behind` 或 `ahead`、秒數一位小數），離開時記一次 `info`；提供依成員位元組序排序的「處於狀態的成員與偏差」清單，以及所有有量測的成員與偏差（給 metric）。常數 `CLOCK_WARN_MS`、`CLOCK_CLEAR_MS`、`CLOCK_CONSECUTIVE`、`CLOCK_KEEP` 寫在 group.rs。驗證：單元測試（用傳入的 `now` 與 `now_ms` 控制時間）涵蓋該需求的其餘六個 scenario：`Member behind by 3 seconds`、`One delayed message does not enter the state`、`Hysteresis`、`Ahead of this host`、`The measurement expires`、`Clocks agree`，並確認 log 只在轉換時各記一次；`tools/docker-test.sh 'cargo test -p emfor-daemon peers::group'` 通過。

## 2. 輸出

- [x] 2.1 [after: 1.2] 寫進 `meta/peers`：`quorum.rs` 的 `Summary` 加 `clock_offsets`（`{"member","offset_ms"}`，依成員排序，空時不寫出，`skip_serializing_if`），tick（`peers/task.rs`）把 `Group` 的清單傳進去組成 summary；`status.rs` 的 `PeerState` 讀端加 `#[serde(default)]` 的 `clock_offsets`，並在沉默 checker 的行之後輸出 `peers: clock of <member> is <n.n> s behind|ahead of this host (...)`；當 `clock_offsets` 筆數等於 `members` 筆數且至少兩個時，改輸出單一行 `peers: clock of this host differs from every member by up to <n.n> s; check this host's clock (...)`。驗證：summary 測試確認有偏差時欄位存在、沒有時完全不出現（state-store「Clock offsets in the peer state」兩個 scenario）；status.rs 文字行測試涵蓋 status-command「Clock offset line in status text」的五個 scenario；`tools/docker-test.sh 'cargo test -p emfor-daemon'` 通過。
- [x] 2.2 [after: 1.2] metric：`peers/stats.rs` 為每個成員存最近的偏差（沒有量測就沒有值），tick 每秒更新；`metrics.rs` 在有 `[peers]` 時輸出 `emfor_peer_clock_offset_seconds{peer="<addr>"}`（gauge，秒，帶正負號，label 用 `label()`），沒有量測的成員不輸出，沒有 `[peers]` 時不輸出這個名稱的系列。驗證：metrics.rs 輸出測試涵蓋 metrics-endpoint「Clock offset metric」三個 scenario（含 `0.012` 與 `-3` 的數字格式）；`tools/docker-test.sh 'cargo test -p emfor-daemon metrics'` 通過。

## 3. 容器驗證

- [x] 3.1 [after: 2.1, 2.2] 新增 `tools/peer-clock-test.sh`（沿用 `tools/peer-mismatch-test.sh` 的三容器設定）：docker 容器共用核心時鐘，不能用 `date -s` 只改一個，所以在測試映像加 libfaketime（`faketime`），用 `faketime -f "-3s"` 啟動成員 C 的 daemon，驗證：C 的時鐘慢 3 秒時，A 在 8 秒內出現 `clock of <C> is 3.0 s behind` 一行、`status --json` 有 `clock_offsets`、metric 約 -3；改為慢 6 秒時，A 的 `stale` 計數上升且偏差仍顯示約 6.0 秒；A 自己用 `faketime -f "+4s"`、B 與 C 正常時，A 只出現一行指向本機；三台時鐘一致時 30 秒內都不出現。若 faketime 在這個映像裡讓 daemon 的 `unix_ms` 取不到偽造時間，改為只留單元測試，並在腳本標頭與手冊（第 12 章）寫明沒有容器驗證。驗證：執行 `tools/peer-clock-test.sh`，輸出 `all checks passed`。

## 4. 文件

- [x] 4.1 [after: 2.1, 2.2] 手冊（docs/manual 與 docs/manual.zh_TW）：第 11 章 stale 說明（約在 Metrics 表前）與 Metrics 表加 `emfor_peer_clock_offset_seconds`；第 12 章 `After a clock jump` 的 peer 條目改為「`emfor status` 在偏差 2 秒起出現一行，超過 5 秒時訊息被丟」，並寫「每個成員都出現時先查本機時鐘」、status 行與 `--json` 的 `peers.clock_offsets` 與 metrics 表；第 16 章 `stale` 那一列加上新的訊號。zh_TW 照 docs/spectra/LANGUAGE.md 與自然用語。驗證：內容審閱對照 design 的輸出格式與 spec 的行文；`python3 tools/check-manual-links.py` 通過。
- [x] 4.2 [after: 1.2] CHANGELOG.md 的 Unreleased 加一筆；docs/spectra/LANGUAGE.md 加「時鐘偏差」（clock offset）一列，定義與避免使用的說法（例如不要和 `stale` 拒絕或 `heartbeat in the future` 混用）。驗證：內容審閱，詞與 spec 一致。

## 5. 收尾

- [x] 5.1 [after: 3.1, 4.1, 4.2] 完整檢查：`tools/docker-test.sh`（不帶參數，含 fmt、clippy、全部測試）通過，且既有的 status、metrics、peer 測試沒有因新欄位、新行或新的 metric 系列而被改動預期。驗證：docker-test.sh 回報全部通過。
