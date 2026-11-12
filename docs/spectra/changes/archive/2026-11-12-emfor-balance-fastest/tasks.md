## 1. 檢查延遲

- [x] 1.1 為每個後端實作 "Check latency" 與 "Store record format version 1" 的 `latency_us` 欄位，依照設計決策「延遲是 up 檢查的耗時」、「以 EWMA 平滑，新樣本權重 0.3」、「後端變成不健康時清空延遲」與「store 欄位 latency_us，格式版本維持 1」：`crates/emfor-daemon/src/schedule.rs` 的後端 task 用單調時鐘量測 `run_check`，把平滑後的延遲存在 `BackendHealth` 旁邊，以 `debug` 記錄每次 up 檢查的原始微秒數，並在 `BackendUpdate` 中送出延遲；writer 儲存 `latency_us`（只有延遲變動時於下一次心跳寫入），DNS 使用的記憶體狀態帶同樣的值；重新載入時隨後端狀態保留或清空。驗證：以 "smoothing" 範例表做單元測試（含 `down`、`error` 列與不健康時清空）、一個 store 測試確認 up 檢查後 `latency_us` 為數字且後端變成不健康後為 `null`、既有的重新載入測試仍通過，以及 `cargo test -p emfor-daemon`。
- [x] 1.2 依設計決策「以 EWMA 平滑，新樣本權重 0.1」把平滑權重改為 0.1，取代 task 1.1 實作的權重 0.3 [after: 1.1]：`crates/emfor-daemon/src/state.rs` 的 `smooth_latency` 計算 `(sample + 9 * previous + 5) / 10`，`smoothing_table` 測試依 "Check latency" 更新後的 "smoothing" 範例（1000、1100、1190）。驗證：`cargo test -p emfor-daemon smoothing` 與 `latency_cleared_on_unhealthy`。
- [x] 1.3 依設計決策「以 EWMA 平滑，新樣本權重 0.05」把平滑權重改為 0.05，取代 task 1.2 實作的權重 0.1 [after: 1.2, 2.1]：`crates/emfor-daemon/src/state.rs` 的 `smooth_latency` 計算 `(sample + 19 * previous + 10) / 20`，`smoothing_table` 測試依 "Check latency" 更新後的 "smoothing" 範例（1000、1050、1098，`down` 與 `error` 列維持 1098）。驗證：`cargo test -p emfor-daemon smoothing` 與 `latency_cleared_on_unhealthy`。

## 2. 實驗

- [x] 2.1 完成「先實驗再實作選擇」所描述的實驗 [after: 1.1]：「第一輪（2026-11-12，36 分鐘）」已記錄在設計的「實驗結果」；第二輪沿用第一輪的設定，再加一台延遲明顯超出進入門檻的後端（例如以 `tc netem` 增加至少 5 ms 的 VM 埠，或回應前先等 5 ms 的 http 後端），至少跑 60 分鐘，並用第一輪保留的腳本，從原始 `debug` 樣本離線評估「daemon 以遲滯決定快組成員」的常數。在「實驗結果」新增「第二輪」，內容包括各後端的中位數與第 95 百分位數、慢後端在快組裡的時間比例、各服務每小時的成員變動次數，以及選定的常數與門檻結果。以內容審閱該節驗證。如果常數改變，先停下來透過 `/spectra-ingest` 更新規格與設計，再做 task 3.1；如果沒通過門檻，change 就停在這裡，並記錄 `fastest` 在 docs/spectra/config.yaml 維持延後。

## 3. Daemon

- [x] 3.1 依照「daemon 以遲滯決定快組成員」與「store 欄位 pool」，實作 "Fast membership decided by the daemon" 與 "Store record format version 1" 的 `pool` 欄位 [after: 1.3, 2.1]：`emfor_format::ServiceRecord` 多 `pool`（非 `fastest` 服務不輸出，缺少時視為空），`Balance` 多 `Fastest`；對 `fastest` 服務，`crates/emfor-daemon/src/writer.rs` 的 writer 保存各後端的「快」旗標，每次狀態或延遲更新以及重新載入後重新計算，把快後端的 id 寫成 `pool`，只有 pool 變動時於下一次心跳寫入，狀態改變時立即連同重新計算的 pool 寫入，並把 pool 同步到記憶體狀態。驗證：成員函式對 "fast membership" 範例每一列的單元測試；writer 對 "Nearly equal backends are fast"、"A fast backend near the edge stays fast"、"Missing latency is not fast" 與 "Pool written for a fastest service" 的測試；以及一個 daemon 測試，確認 `fastest` 服務的 `pool` 列出唯一健康的後端，該後端變成不健康後為空。
- [x] 3.2 在 "Balance methods" 與 "Config validation rules" 下接受 `fastest`，並依「daemon 發布 pool，讀取端在其中輪流」為 A/AAAA 回答實作 "Fastest choice in the daemon" [after: 3.1]：`crates/emfor-daemon/src/config.rs` 接受 `fastest`，錯誤訊息列出全部六個值；`crates/emfor-daemon/src/select.rs` 在 pool 中該 IP 版本的後端之間輪流，否則照 `round-robin` 挑選。驗證：四個 daemon 情境（"Fast backends preferred"、"Empty pool"、"All down open"、"Pool without the family"）的單元測試、新值與 "Unknown value rejected" 情境的設定測試，以及一個 daemon 整合測試：回應會延遲的測試後端在兩台都健康且有延遲後，不會出現在 A 回答中（實際驗證方式見 Change notes）。
- [x] 3.3 依「SRV：快組 priority 0，其他 priority 1」在 "SRV answers" 加入 `fastest` 的對應 [after: 3.2]：pool 成員給 priority 0，其他候選給 priority 1，pool 為空時全部給 0，順序是輪換後的清單依 priority 穩定排序。驗證：一個 "Fastest priorities" 情境的測試，以及一個 pool 為空的測試（實際放在 `crates/emfor-daemon/src/select.rs`，SRV 的單元測試都在這裡），另加 `crates/emfor-daemon/tests/dns.rs` 的整合測試。

## 4. Client

- [x] 4.1 在 Rust client 實作 "Fastest lookups rotate over the daemon's pool" [after: 3.2]：`clients/rust/src/select.rs` 處理 `Fastest` 時，如果 store 未過期且 `pool` 至少有一個 id 對得上後端，就用 round-robin 計數器在 `pool` 列出的後端之間輪流，否則完全照 `round-robin`；不讀 `latency_us`，也不用其他資訊判斷後端。驗證：三個 client 情境的單元測試與 `cargo test -p emfor-client`。
- [x] 4.2 在 "Shared conformance data defines the contract" 加入 `fastest` [after: 4.1]：在 `tests/conformance/v1/cases.json` 新增案例：非空 pool、空 pool、沒有 pool、心跳過期但有 pool、pool 列出不存在的後端 id；需要輪流時用 `expect_counts`，每個案例的 `case` 名稱都以 `fastest` 開頭並說明內容。驗證：Rust 的一致性測試 runner，以及在 Debian 測試 image 中透過 `emfor-conformance` driver 跑 C API 與 Go、Python、Perl、PHP 包裝層，全部 0 失敗。

## 5. 文件與檢查

- [x] 5.1 為 `fastest` 寫文件 [after: 3.3, 4.2]：docs/spectra/LANGUAGE.md 的 `fastest` 列描述由 daemon 以遲滯決定、發布為 `pool` 的快組，不再標示「規劃中」；如果用語表列有 store 欄位，加入 `pool` 與 `latency_us`；docs/spectra/config.yaml 把 `fastest` 從後續規劃移到分配方式說明，並在 change 串接中加上 emfor-balance-fastest；docs/deploy/dns-resolver.md 在 `locality` 旁列出 `fastest` 的 SRV 對應，並說明延遲是從這台主機量測的健康檢查耗時。以對照設計的內容審閱驗證。
- [x] 5.2 執行本機檢查 [after: 5.1]：在 macOS 和 Debian 測試 image 中執行 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 與 `cargo test --workspace --all-features`，並在 Docker 中執行 `tests/resolver/run.sh dnsmasq` 與 `tests/resolver/run.sh unbound`，全部通過；把指令與結果記在這個 task 的完成紀錄。
  - 完成紀錄（2026-11-12）：macOS：`cargo fmt --check` 通過，`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通過，`cargo test --workspace --all-features` 全部通過。Debian image `emfor-clients-test`：fmt 與 clippy 通過；`make -C clients test`（`EMFOR_REQUIRE_DBA=1`）的 C、Go、Python、Perl、PHP 全部通過（PHP 一致性測試：45 個案例、67 次查詢、0 失敗）；`cargo test --workspace --all-features` 跑兩次各失敗一個測試，每次是未修改的 exec 檢查器中不同的測試（先是 `background_child_is_killed_after_normal_exit`，再來是 `large_output_does_not_block`），而第一個單獨跑五次、`cargo test -p emfor-daemon --lib`（137 個測試）跑三次都全部通過，所以這是整個 workspace 平行執行時對時序敏感的問題，不是這個 change 造成的。`tests/resolver/run.sh dnsmasq` 與 `tests/resolver/run.sh unbound`：通過。

## Change notes

- 已取代（未完成）：先前待辦的「3.1 接受 `fastest` ……fastest 的快組」、「3.2 快組的 SRV 對應」、「4.1 client 從 `latency_us` 算出快組」已改寫為上方的 3.1–3.3 與 4.1，因為設計決策「daemon 以遲滯決定快組成員」取代了「快組：與最快者相差 1.5 倍或 1 ms 以內」，把規則移到 daemon。
- 已取代（未完成）：第二版 3.1、4.1、4.2 規劃的每個後端一個 `fast` 旗標，改成服務層級的 `pool`，因為「daemon 發布 pool，讀取端在其中輪流」與「store 欄位 pool」取代了「client 與 DNS 在快後端之間以 round-robin 挑選」與「store 欄位 fast」：讀取端只收到可挑選的集合。
- 已取代（已完成，已補正）：task 1.2 實作了權重 0.1；第二輪實驗（task 2.1）顯示權重 0.1 在 1 秒間隔下未通過門檻第 (2) 項，使用者選定 0.05，由 task 1.3 依「以 EWMA 平滑，新樣本權重 0.05」修改。
- 驗證方式調整（3.2、3.3）：「回應會延遲的測試後端不會出現在 A 回答中」這個 daemon 整合測試在 macOS 上分不出後端，因為 loopback 只有 127.0.0.1，A 回答又不帶埠號。改由以下涵蓋：`crates/emfor-daemon/tests/daemon.rs` 的 `fastest_pool_excludes_slow_backend`（加延遲的 http 後端不會進入 store 的 `pool`）、`crates/emfor-daemon/src/select.rs` 的 A/AAAA 單元測試，以及 task 3.3 的 SRV 整合測試 `fastest_srv_gives_slow_backend_priority_1`（加延遲的後端拿到 priority 1；SRV 回答帶埠號）。
- apply 期間新增規格：delta spec service-config 修改 "Config validation rules"，讓允許的 `balance` 值與 "least-conn" 錯誤訊息都包含 `fastest`，對應 task 3.2 的設定變更；沒有它，封存後主規格會留著五個值的清單。
- 改寫為繁體中文（2026-11-12）：使用者讓 `.spectra.yaml` 使用 `locale: tw`，proposal、design、tasks 改寫成中文，設計決策標題隨之改為中文，task 內容與完成狀態不變；specs 依 Spectra 規定維持英文，所以引用 requirement 與 scenario 時保留英文名稱。
- review 後修正（2026-11-12）：`/spectra-review` 發現 `http` 檢查的第一個樣本包含建立 HTTP client 的時間。`Checker` trait 新增預設不做事的 `prepare(target)`，`HttpChecker` 在其中建立 client，`crates/emfor-daemon/src/schedule.rs` 在計時前呼叫它；設計決策「延遲是 up 檢查的耗時」已補充說明。以 `forget_releases_backend_client` 測試（`prepare` 先建好 client、`check` 沿用）與 `cargo test -p emfor-daemon` 驗證。
