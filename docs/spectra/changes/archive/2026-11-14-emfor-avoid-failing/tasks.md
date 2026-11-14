## 1. 共用候選規則

- [x] 1.1 在 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all` 實作三層候選（決策「三層候選放在共用的 healthy_or_all」）：有通過的 backend（`state` 為 `healthy` 且 `reason` 為 `null`，見決策「正在失敗的定義用 reason 是否為 null」）時只回通過的；否則回全部健康的；都沒有時照 `on_all_down`；`trust_health` 為 false 時不看 `reason`。函式簽名不變，doc comment 改寫成三層。驗證：擴充 `healthy_or_all_table` 單元測試，涵蓋 client-lookup 範例表「candidates by state, reason and heartbeat」的六列；先暫時拿掉第一層，確認新列會失敗，再恢復；`cargo test -p emfor-format` 通過。

## 2. daemon 的 DNS 選擇

- [x] 2.1 [after: 1.1] 確認 requirement「Candidate backends per address family」在 daemon 成立：A/AAAA 只在通過的 backend 中挑選，全部正在失敗時在健康的 backend 中挑選。驗證：在 `crates/emfor-daemon/src/select.rs` 的測試模組新增 `failing_backends_are_skipped` 與 `every_healthy_backend_failing`（對應 backend-selection 的兩個新 scenario），並把範例表新增的兩列加進 `candidate_sets_example`；暫時拿掉 1.1 的第一層時這些測試失敗；`cargo test -p emfor-daemon --lib select` 通過。
- [x] 2.2 [after: 1.1] 確認 requirement「SRV answers」依決策「SRV 與 A/AAAA 用同一條規則」運作：有通過的 backend 時，SRV 回答不列正在失敗的 backend；全部正在失敗時照 `balance` 列出健康的 backend，priority 編法不變。驗證：在 `crates/emfor-daemon/src/select.rs` 新增 `srv_failing_backend_left_out` 與 `srv_every_healthy_backend_failing`（對應 dns-server 的兩個新 scenario，第二個要斷言 failover 的 priority 0、1）；暫時拿掉 1.1 的第一層時會失敗；`cargo test -p emfor-daemon --lib select` 通過。

## 3. client 與 conformance 資料

- [x] 3.1 [after: 1.1] 確認 requirement「Candidates follow health and on_all_down」在 client 成立。在 `tests/conformance/v1/cases.json` 新增四個案例，對應 client-lookup 的三個新 scenario 再加一個 `round-robin` 案例：
  - failover 跳過正在失敗的 backend：`expect` `10.0.1.6:5432`；
  - round-robin 只輪流通過的 backend：用 `expect_counts`，正在失敗的 backend 次數為 0、不出現；
  - 全部正在失敗、`on_all_down = "closed"`：用 `expect_all_of`，兩台正在失敗的 backend 都出現，unhealthy 那台不出現；
  - store 過期、`on_all_down = "open"`：`reason` 不影響結果，failover 回第一台。

  另在 `clients/rust/src/select.rs` 的 `candidates_follow_health_and_on_all_down` 加入對應斷言。
  驗證：
  - `cargo test -p emfor-client --all-features` 通過（conformance runner 與 C API runner 需要 `testing`、`capi` feature 才會編譯），兩者都跑到新案例；
  - `cargo test -p emfor-fixture` 的覆蓋檢查通過；
  - 暫時拿掉 1.1 的第一層時，新案例失敗。
- [x] 3.2 [after: 3.1] 確認各語言 wrapper 也通過新案例（各 `balance` 的挑選方式不變、只換候選，見決策「balance 方法不變，只換候選」）。驗證：在 Debian 容器 `emfor-clients-test` 中跑 `make -C clients test`（C、Go、Python、Perl、PHP 各自的 Makefile 都經 `$(EMFOR_CONFORMANCE)` 跑共用的 conformance 資料），每個介面回報 0 個失敗。

## 4. 文件與發布紀錄

- [x] 4.1 [after: 1.1] 手冊說明正在失敗的 backend。在 `docs/manual/05-backend-selection.md` 與 `docs/manual.zh_TW/05-backend-selection.md` 開頭的候選說明中加入：健康但最近一次檢查失敗（`reason` 有值）的 backend，在還有其他通過的 backend 時不被選到；全部健康的 backend 都在失敗時照常使用。另外說明：
  - client 讀 store 有最多約 1 秒的延遲，DNS 沒有；
  - `sticky` 的 key 在失敗期間會暫時改對到別台。

  兩種語言的內容一致。驗證：人工對照兩份文件與 client-lookup、backend-selection 的新 scenario。
- [x] 4.2 [after: 4.1] 在 `CHANGELOG.md` 的未發布區段加入一條，說明 client 與 DNS 優先選最近一次檢查通過的健康 backend，store 格式不變。驗證：人工檢查條目位於未發布區段，並與 proposal 的 What Changes 相符。

## 5. 整體檢查

- [x] 5.1 [after: 2.1, 2.2, 3.2, 4.2] 整個 workspace 在 macOS 與 Debian 容器都通過 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`。驗證：上述指令的輸出 0 個錯誤、0 個失敗。
