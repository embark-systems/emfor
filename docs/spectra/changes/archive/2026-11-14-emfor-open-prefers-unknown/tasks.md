## 1. 共用候選規則

- [x] 1.1 依決策「unknown 優先的兩層放在 healthy_or_all 的 open 分支」、「unknown 之間依 reason 是否為 null 分層」與「store 過期時不分層」，在 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all` 實作新行為：store 新鮮、`open`、沒有健康 backend 時，候選依序是 `unknown` 且 `reason` 為 `null` 的、全部 `unknown` 的、全部 backend；`trust_health` 為 false 時直接回全部 backend；`closed` 不變。函式簽名不變，doc comment 改寫成五層。驗證：
  - 在 `healthy_or_all_table` 單元測試加入 client-lookup 範例表「candidates by state, reason and heartbeat」新增與修改的列，並把現有的 `none_healthy` 在 `open` 下的預期改成只有 `unknown` 那台；
  - 先暫時拿掉新的兩層，確認新列與改過的列失敗，再恢復；
  - `cargo test -p emfor-format` 通過。

## 2. daemon 的 DNS 選擇

- [x] 2.1 [after: 1.1] 讓 requirement「Candidate backends per address family」的新行為在 daemon 成立：`open` 且某家族沒有健康 backend 時，A/AAAA 先在該家族的 `unknown` backend 中挑選，依家族分開判斷。驗證：
  - `crates/emfor-daemon/src/select.rs` 的 `candidate_sets_example` 改成新範例表：`unhealthy` 加 `unknown` 的 `open` 列預期只有 10.0.0.2，並加入另外兩列新列；
  - 新增測試 `open_prefers_unknown_backends`，對應 backend-selection 的 scenario「Open service prefers unknown backends」；
  - 暫時拿掉 1.1 的新兩層時，這些測試失敗；
  - `cargo test -p emfor-daemon --lib select` 通過。
- [x] 2.2 [after: 1.1] 讓 requirement「SRV answers」的新候選在 daemon 成立：`open` 且沒有健康 backend 時，SRV 只列 `unknown` 的 backend（有通過的時只列通過的）。驗證：
  - `srv_health_across_families_and_all_down` 中 `open` 的預期，從兩筆改成只有 `[2001:db8::11]:8080` 一筆；
  - 新增斷言或測試，對應 dns-server 的 scenario「All down open with an unknown backend」；
  - 保留 scenario「All down open」（全部 `unhealthy` 時列出全部）的斷言；
  - 暫時拿掉 1.1 的新兩層時失敗；
  - `cargo test -p emfor-daemon --lib select` 通過。

## 3. client 與 conformance 資料

- [x] 3.1 [after: 1.1] 讓 requirement「Candidates follow health and on_all_down」的新行為在 client 成立。
  - 修改 `tests/conformance/v1/cases.json` 既有案例「open service with no healthy backend picks from all」：`expect` 改為 `10.0.1.6:5432`，案例名稱改成說明 `unknown` 優先。
  - 新增四個案例，對應 client-lookup 的 scenario：
    - 「Open service prefers a passing unknown backend」：`expect_counts` 只有 `10.0.1.7:5432`；
    - 「Open service with every unknown backend failing」：`expect_counts`，兩台 `unknown` 各半，`unhealthy` 那台不出現；
    - 「Open service with only unhealthy backends」：`expect_counts`，兩台各半；
    - `closed` 而有 `unknown` 的服務仍回 `ALL_DOWN`。
  - `clients/rust/src/select.rs` 的 `candidates_follow_health_and_on_all_down` 中，failover `open` 的預期改成 `10.0.1.6:5432`。

  驗證：
  - `cargo test -p emfor-client --all-features` 通過，Rust API 與 C API 的 conformance runner 都跑到這些案例；
  - `cargo test -p emfor-fixture` 通過；
  - 暫時拿掉 1.1 的新兩層時，新案例與修改的案例失敗。
- [x] 3.2 [after: 3.1] 確認各語言 wrapper 也通過新的與修改的案例。驗證：在 Debian 容器 `emfor-clients-test` 中跑 `make -C clients test`，C、Go、Python、Perl、PHP 每個介面都回報 0 個失敗。

## 4. 文件與發布紀錄

- [x] 4.1 [after: 1.1] 手冊說明 `open` 的新順序：沒有健康 backend 時，先用最近一次檢查通過的 `unknown` backend，其次是全部 `unknown`，最後才是全部 backend；store 過期時仍用全部；`closed` 不變。要修改的地方（英文與中文各一份，兩種語言內容一致）：
  - `docs/manual/01-concepts.md` 說明 `open` 的段落與摘要表；
  - `docs/manual/05-backend-selection.md` 的「When no backend is healthy」；
  - `docs/manual/07-dns.md` 的 SRV 候選說明；
  - `docs/deploy/dns-resolver.md` 的 SRV 段落。

  驗證：人工對照三份 delta spec 與 design 的 Implementation Contract。
- [x] 4.2 [after: 4.1] 在 `CHANGELOG.md` 未發布區段加入條目，說明 `open` 在沒有健康 backend 時優先用 `unknown` backend，store 格式不變，舊版 client 仍從全部 backend 中挑。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 5. 整體檢查

- [x] 5.1 [after: 2.1, 2.2, 3.2, 4.2] 依 RELEASING.md 第 4 步，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`。驗證：所有指令 0 個錯誤、0 個失敗。
