## 1. 量測

- [x] 1.1 依決策「先量測，再決定」，在 `clients/rust/src/select.rs` 以虛擬節點版的 `rendezvous_index` 草稿（尚未接入 `pick`），用 release build 量測 10 個候選、權重各 1、10、100 時每次挑選的平均時間（各 100000 次，取三次中位數），把數字寫進本 change 的 design.md「先量測，再決定」一節。驗證：數字已記錄；10×100 不超過 100 µs 才繼續，否則停下回報，不進入 2.1。

## 2. 選擇算法

- [x] 2.1 [after: 1.1] 依 client-lookup requirement「Sticky uses weighted rendezvous hashing」與決策「用虛擬節點做加權 rendezvous」，讓 `select::rendezvous_index` 接受 `(id, weight)`，`pick` 的 `(Balance::Sticky, Some(k))` 傳入候選的 `weight`（不經慢啟動調整）。驗證：`select.rs` 單元測試涵蓋 Example「weighted hash vectors」「weighted picks」、既有的「rendezvous hash vectors」「rendezvous picks」、Scenario「Raising a weight only moves keys to that backend」的 499 與 151，以及權重全為 1 時 `key-0` 到 `key-999` 與只算 `h_0` 的結果逐一相同；暫時改回只算 `h_0` 時加權的測試失敗。
- [x] 2.2 [after: 2.1] 依決策「公開函式加上權重版本」，在 `clients/rust/src/lib.rs` 新增 `rendezvous_pick_weighted`，`rendezvous_pick` 行為不變。驗證：新增的單元測試以 Example「weighted picks」的一列呼叫 `rendezvous_pick_weighted` 並通過；`sticky-v1.json` 的 Rust runner 仍通過。

## 3. Conformance

- [x] 3.1 [after: 2.1] 在 `tests/conformance/v1/cases.json` 新增加權 sticky 案例（store 版本 `2`，backend 帶 `weight`）：Example「weighted picks」的六列，各一個 lookup 帶 `affinity_key` 並 `expect` 該 backend；既有 sticky 案例不改。更新 `tests/conformance/v1/README.md` 說明加權案例。驗證：`cargo test -p emfor-client --all-features` 通過；在 Debian 容器 `emfor-clients-test` 跑 `make -C clients test`，C、Go、Python、Perl、PHP 都回報 0 個失敗；暫時改回只算 `h_0` 時 Rust conformance 有案例失敗。

## 4. 文件與發布紀錄

- [x] 4.1 [after: 2.1] 手冊（英文 `docs/manual` 與中文 `docs/manual.zh_TW` 內容一致）：
  - 第 3 章 backend 表格的 `weight` 列：用於 `balance = "weighted"` 與 `"sticky"`；
  - 第 5 章 sticky 一節：加權的算法（虛擬節點、`#i`）、權重全為 1 時結果不變、調高權重只把 key 移到該台、不受慢啟動影響。

  驗證：人工對照 delta spec 與 design；中英文一致。
- [x] 4.2 [after: 4.1] 依決策「直接改 sticky，不新增 balance 名稱」，在 `CHANGELOG.md` 未發布區段加入條目（標為行為變更，不新增 `balance` 名稱）：`sticky` 依 `weight` 分配 key；權重全為 1 時不變；已設定非 1 權重的 sticky 服務升級後部分 key 會換 backend，新舊 client 並存時選擇不同，同一服務的 client 應一起升級；附上 1.1 的量測結果。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 5. 整體檢查

- [x] 5.1 [after: 2.2, 3.1, 4.2] 依 RELEASING.md，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`。驗證：所有指令 0 個錯誤、0 個失敗。

## 6. 權重上限（review 後追加）

- [x] 6.1 依 design「權重超過 100 時當成 100」與 Scenario「Weights above 100 count as 100」，`select::rendezvous_score` 把超過 100 的權重當成 100、0 當成 1。驗證：`select.rs` 單元測試以 `key-795` 確認權重 101 與 `u32::MAX` 的分數等於權重 100 的分數；`cases.json` 新增案例「sticky counts weights above 100 as 100」含 Example「weights above 100」三列；拿掉上限時權重 101 的單元測試與 conformance 失敗；macOS 與 Debian 依 RELEASING.md 的完整檢查與 `make -C clients test` 通過。
