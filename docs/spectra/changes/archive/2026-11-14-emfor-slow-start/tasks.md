## 1. 格式與共用計算

- [x] 1.1 在 `crates/emfor-format/src/lib.rs` 新增 `ServiceRecord.slow_start_ms: u64` 與 `BackendRecord.recovered_at: Option<u64>`（皆 serde default），JSON 欄位順序符合 state-store requirement「Store record format version 1」的 record shape 範例：`slow_start_ms` 在 `all_down` 之後，`recovered_at` 在 `latency_us` 之後。在 `crates/emfor-format/src/rules.rs` 新增 `effective_weight(b, base, slow_start_ms, now_ms) -> u32` 與判斷候選中是否有 backend 正在加量的輔助函式，實作 backend-selection requirement「Slow start effective weight」（決策「有效權重：10 級線性加量」）。驗證：
  - lib.rs 測試：record shape 範例逐字相符；缺少新欄位的舊 JSON 讀成 0 與 `None`；
  - rules.rs 測試涵蓋 backend-selection requirement「Slow start effective weight」範例表的九列；
  - 把 step 固定為 10 時新測試失敗；
  - `cargo test -p emfor-format` 通過。

## 2. daemon 的設定與 store

- [x] 2.1 [after: 1.1] 依 service-config requirement「Slow start setting」在 `crates/emfor-daemon/src/config.rs` 加入 `slow_start`（服務表格與 `[defaults]`，預設 `"0s"`，`0s` 或 1s 到 1h），並讓 daemon 寫入的服務記錄帶 `slow_start_ms`，reload 改值後立即更新。驗證：
  - config.rs 測試涵蓋範例表「slow_start validation」的八列，以及 scenario「Default and override」；
  - 錯誤訊息以 `services."api.svc.emfor".slow_start:` 開頭；
  - writer 測試：reload 把 `slow_start` 從 `"0s"` 改成 `"30s"` 後，記錄的 `slow_start_ms` 為 30000；
  - `cargo test -p emfor-daemon --lib config writer` 通過。
- [x] 2.2 [after: 2.1] 依 state-store requirement「Recovery time in the store」、決策「daemon 記錄恢復時間，挑選端自己算進度」與決策「recovered_at 的寫入規則」，在 `crates/emfor-daemon/src/writer.rs` 的 `update` 依 transition 設定 `recovered_at`，並讓 `fresh_backend` 與 reload 重設時為 `None`、未重設時保留。新值和狀態改變在同一次 commit 寫入，`live` 中給 DNS 用的記錄也帶這個值。驗證：
  - writer.rs 測試涵蓋範例表「recovered_at after each transition」的四列，以及 reload 保留與重設兩種情況；
  - 拿掉設定邏輯時測試失敗；
  - `cargo test -p emfor-daemon --lib writer` 通過。

## 3. daemon 的 DNS 挑選

- [x] 3.1 [after: 1.1] 依 backend-selection requirement「Slow start choice in the daemon」與決策「加量期間改用加權挑選，其他時候不變」，讓 `crates/emfor-daemon/src/select.rs` 的挑選取得現在時間（新增 `select_at`／`srv_targets_at` 接收 Unix 毫秒，原本的 `select`／`srv_targets` 以 `schedule::unix_ms()` 呼叫它們，DNS 呼叫端不變；測試傳入固定時間），候選中有 backend 正在加量時，用 `smooth_weighted` 依有效權重挑選；沒有時完全照舊。驗證：
  - 新增 `slow_start_ramping_share`、`slow_start_failover_unchanged`、`slow_start_weighted_ramping`，對應三個 scenario 的次數；
  - 現有 select 測試不需修改即通過；
  - 把 step 固定為 10 時新測試失敗；
  - `cargo test -p emfor-daemon --lib select` 通過。
- [x] 3.2 [after: 3.1] 依 dns-server requirement「SRV weights during slow start」，讓 `srv_targets` 在有候選正在加量時以有效權重填 weight，`failover` 例外。驗證：
  - 新增 `srv_slow_start_round_robin`、`srv_slow_start_weighted`、`srv_slow_start_failover`，對應三個 scenario 的 SRV 記錄；
  - 把 step 固定為 10 時前兩個測試失敗；
  - `cargo test -p emfor-daemon --lib select` 與 `cargo test -p emfor-daemon --test dns` 通過。

## 4. client 與 conformance

- [x] 4.1 依決策「conformance 測試用相對時間」，讓 `tools/emfor-fixture/src/lib.rs` 的 `store_entries` 在 backend 物件有 `recovered_ago_ms` 時寫入 `recovered_at` = 現在 − 該值，並移除這個鍵；`tests/conformance/v1/README.md` 說明這個欄位。驗證：fixture 單元測試讀回 `recovered_at` 落在 [寫入前的現在 − ago, 寫入後的現在 − ago] 之間，且沒有 `recovered_ago_ms` 鍵；`cargo test -p emfor-fixture` 通過。
- [x] 4.2 [after: 1.1] 依 client-lookup requirement「Slow start in lookups」，讓 `clients/rust/src/select.rs` 的 `pick`（與 `clients/rust/src/client.rs` 傳入現在時間與是否過期）在新鮮的 store 中，候選有 backend 正在加量時，依有效權重的累積區間挑選；`failover` 與有 key 的 `sticky` 不變。驗證：
  - select.rs 單元測試涵蓋五個 scenario 的次數；
  - 把 step 固定為 10 時新測試失敗；
  - `cargo test -p emfor-client --all-features` 通過。
- [x] 4.3 [after: 4.1, 4.2] 在 `tests/conformance/v1/cases.json` 新增五個案例，分別對應 client-lookup 的五個 slow start scenario。round-robin 與 weighted 用 `expect_counts`，`slow_start_ms` 100000、`recovered_ago_ms` 25000（step 3）。驗證：
  - `cargo test -p emfor-client --all-features`（Rust 與 C API runner）與 `cargo test -p emfor-fixture`（覆蓋檢查）通過；
  - 把 step 固定為 10 時新案例失敗。
- [x] 4.4 [after: 4.3] 確認各語言 wrapper 通過新案例。驗證：在 Debian 容器 `emfor-clients-test` 中跑 `make -C clients test`，C、Go、Python、Perl、PHP 每個介面回報 0 個失敗。

## 5. 文件

- [x] 5.1 [after: 2.1] 手冊說明 slow start：
  - `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md` 的 `[defaults]` 與 `[services."name"]` 列出 `slow_start`；
  - 兩種語言的 `05-backend-selection.md` 新增一節，說明何時加量、10 級線性、哪些方法套用、單台與全部同時恢復時沒有效果、舊版 client 不加量；
  - `examples/emfor.toml` 加一行註解範例。

  驗證：人工對照 design 的 Implementation Contract 與五份 delta spec；兩種語言內容一致。
- [x] 5.2 [after: 5.1] 在 `CHANGELOG.md` 未發布區段加入 slow start 的條目，說明 store 新增 `slow_start_ms` 與 `recovered_at`、格式版本不變、舊版 client 不加量。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 6. 整體檢查

- [x] 6.1 [after: 2.2, 3.2, 4.4, 5.2] 依 RELEASING.md 第 4 步，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings`、`cargo test --workspace --all-features`；並以 `-max_total_time=60` 跑 `config` 與 `peers` fuzz target，確認新欄位沒有破壞 fuzz 建置。驗證：所有指令 0 個錯誤、0 個失敗，`fuzz/artifacts/` 沒有新檔。
