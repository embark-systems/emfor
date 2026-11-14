## 1. 格式與共用規則

- [x] 1.1 依 state-store requirement「Store record format version 2」與決策「格式版本升到 2，讀取端接受 1 與 2」，在 `crates/emfor-format/src/lib.rs` 新增 `BackendRecord.backup: bool`（`#[serde(default)]`，序列化在 `site` 之後），把 `SCHEMA_VERSION` 改為 2，並新增讀取端共用的版本判斷函式（`1` 與 `2` 可讀）。補上所有 `BackendRecord` 結構字面值的 `backup: false`。驗證：
  - 更新 record shape 的預期 JSON，加入 `"backup":false`；
  - 新增 `backup` 的序列化與「缺少時讀作 false」測試；
  - 新增版本判斷測試：`1`、`2` 可讀，`0`、`3`、空字串不可讀；
  - `cargo test -p emfor-format` 通過。
- [x] 1.2 [after: 1.1] 依決策「備援層插在通過與正在失敗之間」，在 `rules::healthy_or_all` 實作新的順序：通過的主要 → 通過的備援 → 健康的主要 → 健康的備援 → `on_all_down`（`open` 的 `unknown` 優先規則不分主要與備援，見決策「open 的全部 down 不分主要與備援」）。函式簽名不變，doc comment 更新。驗證：
  - `healthy_or_all_table` 加入 client-lookup 範例表「candidates by state, reason and heartbeat」的五列新列；
  - 暫時拿掉兩個備援層（把備援當主要），確認新列失敗後恢復；
  - `cargo test -p emfor-format` 通過。

## 2. daemon

- [x] 2.1 [after: 1.1] 依 service-config requirement「Host site and backend weight and site」與決策「backup 是設定值，不影響健康狀態」，在 `crates/emfor-daemon/src/config.rs` 的 backend 表格接受 `backup`：
  - 不是布林值時報 `backup must be true or false`；
  - 全部都是備援時，在服務的 `backends` 路徑報 `at least one backend must not be a backup`。

  writer 的 `fresh_backend` 與 reload 要把 `backup` 寫進 store。reload 改變 `backup` 時，該值套用，backend 保留健康狀態。
  驗證：
  - config 測試涵蓋範例表「backup values」的四列與 scenario「Only backup backends」；
  - writer 測試確認 store 有 `"backup":true` 與 `meta/schema_version` 為 `2`（原本斷言 `"1"` 的測試改成 `"2"`），以及 reload 改 `backup` 後狀態保留；
  - `cargo test -p emfor-daemon` 通過。
- [x] 2.2 [after: 1.2, 2.1] 讓 requirement「Candidate backends per address family」與「SRV answers」的備援層在 daemon 成立（決策「SRV 與 A/AAAA 用同一條規則」：SRV 只列目前那一層，priority 編法不變）。驗證：
  - `crates/emfor-daemon/src/select.rs` 新增 backend-selection 兩個新 scenario 的測試（「Backup used while no primary passes」「Passing primary wins over a backup」），並把範例表「candidate sets」的四列新列加入 `candidate_sets_example`；
  - 新增 SRV 測試，對應 scenario「Backups listed only while no primary passes」，含 failover priority 0、1，以及主要恢復後只剩主要；
  - 暫時拿掉備援層時這些測試失敗；
  - `cargo test -p emfor-daemon --lib select` 通過。
- [x] 2.3 [after: 1.1] 依 status-command requirement「Status reads the store read-only」，讓 `crates/emfor-daemon/src/status.rs` 用 1.1 的版本判斷函式，接受版本 `1` 與 `2`；其他版本印出 `store format version <n>; this emfor reads versions 1 and 2` 並 exit 2。驗證：
  - status 測試涵蓋版本 1 可讀、版本 2 可讀、版本 3 exit 2 並印出該訊息；
  - 暫時只接受版本 2 時，版本 1 的測試失敗；
  - `cargo test -p emfor-daemon` 通過。

## 3. client 與 conformance

- [x] 3.1 [after: 1.2] 依 client-lookup requirement「Lookup failures use four shared error codes」與「Candidates follow health and on_all_down」，讓 `clients/rust/src/client.rs` 用 1.1 的版本判斷函式，接受版本 `1` 與 `2`；錯誤訊息改為 `this client reads versions 1 and 2`。
  - 在 `tests/conformance/v1/cases.json`：
    - 把既有案例「schema_version 2 wins over a missing service」改成版本 `3`；
    - 新增版本 `2` 的備援案例，對應 scenario「Backup takes over while no primary passes」「Primary passes again」，以及 `failover` 在備援層中回傳第一台通過的備援；
    - 新增一個版本 `2` 而不含備援的一般案例。既有的版本 `1` 案例保留，用來證明版本 1 仍可讀。
  - `tests/conformance/v1/README.md` 說明 `store.schema_version` 可以是 `1` 或 `2`。

  驗證：
  - `cargo test -p emfor-client --all-features` 與 `cargo test -p emfor-fixture` 通過；
  - 暫時拿掉備援層時，備援案例失敗；
  - 暫時只接受版本 2 時，版本 1 的案例失敗。
- [x] 3.2 [after: 3.1] 確認各語言 wrapper 通過新的案例。驗證：在 Debian 容器 `emfor-clients-test` 跑 `make -C clients test`，C、Go、Python、Perl、PHP 都回報 0 個失敗。

## 4. 文件與發布紀錄

- [x] 4.1 [after: 2.1] 手冊說明備援 backend（英文 `docs/manual` 與中文 `docs/manual.zh_TW` 內容一致）：
  - 第 3 章：backend 表格的 `backup`、「不能全部都是備援」的驗證、reload 時套用；
  - 第 5 章：新增一節，說明備援接手的順序、各 `balance` 在選出的那一層裡運作、`open` 全部 down 時不分主要與備援；
  - 第 6 章：格式版本 2，以及升級順序（先 client 後 daemon）；
  - 第 7 章：SRV 只列目前那一層；
  - `examples/emfor.toml`：加一個 `backup = true` 的註解範例。

  驗證：
  - 人工對照六份 delta spec 與 design 的 Implementation Contract；
  - `emfor check-config` 讀取 `examples/emfor.toml`，以及把範例註解取消後的版本，都通過。
- [x] 4.2 [after: 4.1] 在 `CHANGELOG.md` 未發布區段加入條目，標為 **BREAKING**：store 格式版本 2、新 client 讀 1 與 2、舊 client 讀到新 store 回 `INCOMPATIBLE`、先升級 client。另一條說明 `backup` 的行為。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 5. 整體檢查

- [x] 5.1 [after: 2.2, 2.3, 3.2, 4.2] 依 RELEASING.md 第 4 步，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`；並以 `-max_total_time=60` 跑 `config` fuzz target。驗證：所有指令 0 個錯誤、0 個失敗，`fuzz/artifacts/` 沒有新檔。

## 6. verify 與 review 的修正

- [x] 6.1 [after: 5.1] 依決策「fastest 的 pool 依層分開計算」與 backend-selection requirement「Fast membership decided by the daemon」，讓 `crates/emfor-daemon/src/state.rs` 的 `fast_pool` 對主要與備援各自取最低延遲。驗證：`state.rs` 的 `pool_per_tier_primaries_and_backups`、`select.rs` 的 `fastest_with_a_fast_backup_keeps_preferring_fast_primaries`，以及 conformance 案例「fastest with a faster backup: the pool holds the fastest of each tier」通過；改回單一最低值時前兩者失敗，把案例的 pool 換成舊算法的結果時案例失敗。
- [x] 6.2 [after: 6.1] 修正文字：client-lookup 情境「Schema check wins over a missing service」改為版本 `3`；state-store requirement 補上逗號；第 7 章（英文與中文）寫明 SRV 只列選出的那一層；第 5 章與 CHANGELOG 說明 `fastest` 依層計算。驗證：`spectra validate emfor-backup-backends` 通過，人工對照中英文一致。
- [x] 6.3 [after: 6.1] 在 `crates/emfor-daemon/tests/daemon.rs` 新增整合測試：全部 backend 都是備援時，`emfor check-config` exit 2 並印出 `at least one backend must not be a backup`。驗證：該測試通過；拿掉檢查時失敗。
- [x] 6.4 [after: 6.2, 6.3] 重跑 5.1 的整體檢查，以及 Debian 容器的 `make -C clients test`。驗證：0 個錯誤、0 個失敗，`fuzz/artifacts/` 沒有新檔。
