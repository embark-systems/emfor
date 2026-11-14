## 1. 格式與加密

- [x] 1.1 依決策「用 RustCrypto 的 chacha20poly1305 與 hkdf」，在 `crates/emfor-daemon/Cargo.toml` 加入與 `sha2` 0.11 相容（同一個 `digest` 主版本）的 `chacha20poly1305` 與 `hkdf`，鎖定版本並更新 `Cargo.lock`。驗證：macOS 與 Debian 容器 `emfor-clients-test` 都能 `cargo build -p emfor-daemon`，`RUSTFLAGS="--cfg fuzzing"` 的 clippy 也通過。
- [x] 1.2 [after: 1.1] 依 peer-group requirement「Peer message authentication and replay protection」與決策「EMP2：ChaCha20-Poly1305，金鑰由 HKDF 導出」，在 `crates/emfor-daemon/src/peers.rs` 實作 `EMP2` 的編碼與解碼：`Keys` 為每個 secret 保存導出的金鑰；`decode` 依前綴分流，`EMP2` 依 design 的順序計數 `malformed`、`auth`、`malformed`。`EMP1` 行為不變。驗證：
  - 單元測試涵蓋 Example「acceptance checks」中所有 `EMP2` 列（33 與 1201 bytes、nonce 被改、前一個 secret 90 秒、不同 secret、解出非 JSON、`EMP3` 前綴），以及往返與 80 個 tag 的 heartbeat 不超過 1200 bytes；
  - 已知答案測試：固定 secret、nonce 與 payload 產生的 `EMP2` bytes 與用 Python `cryptography` 獨立算出的值相同（計算腳本與結果寫在測試註解）；
  - 每個新測試先確認拿掉對應行為（例如不驗標記、不用 AAD）時失敗。

## 2. 設定與送出

- [x] 2.1 [after: 1.2] 依 requirement「Peer group configuration」「Peer listen requires restart」與決策「兩種格式都接受，送出哪一種由 encrypt 決定」「預設 encrypt = true」，在 `crates/emfor-daemon/src/config.rs` 接受 `[peers] encrypt`（錯誤 `encrypt must be true or false`），送出端依它選格式，reload 修改後從下一則訊息生效；`encrypt = true` 接受 `EMP1` 時每個來源每 60 秒最多一行 `warn`。驗證：
  - config 測試涵蓋 Example「peer config validation」新增的兩列；
  - `crates/emfor-daemon/tests/peers.rs` 新增：兩個 daemon 都 `encrypt = true` 時互相在線且抓到的 datagram 以 `EMP2` 開頭、明文中找不到 `site`（Scenario「Encrypted heartbeat」）；一個 `true` 一個 `false` 時互相在線且 `true` 那邊記一行 `warn`（Scenario「Mixed formats during an upgrade」）；reload 把 `encrypt` 從 `false` 改為 `true` 後送出 `EMP2`；
  - 每個新測試先確認拿掉對應行為時失敗。

## 3. Fuzz

- [x] 3.1 [after: 1.2] 讓 `fuzz/fuzz_targets/peers.rs`（經 `peers::fuzz_decode`）也涵蓋 `EMP2`：以固定的測試 secret 解碼，第一個 byte 決定把其餘資料當成 `EMP1` 或 `EMP2` 的 datagram，並以有效的 `EMP2` heartbeat 作為種子。驗證：`cd fuzz && cargo +nightly fuzz run -O peers -- -max_total_time=60` 沒有新 artifact，且 coverage 輸出顯示走到解密之後的 JSON 解析（以種子確認）。

## 4. 文件與發布紀錄

- [x] 4.1 [after: 2.1] 文件（英文與中文內容一致）：
  - `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md`：`[peers]` 表格加入 `encrypt`；`secret_file` 改為「驗證並加密每則訊息」；升級新舊並存的 peer group 時先設 `encrypt = false`，全部升級後改為 `true` 並 reload；沒有前向保密，建議定期換 secret；
  - `docs/manual/08-operations.md` 與 `docs/manual.zh_TW/08-operations.md`：`auth` 的意義包含解密失敗，以及送明文的 `warn`；
  - `docs/deploy/daemon.md`：metrics 表 `auth` 的說明同上；
  - `examples/emfor.toml`：註解掉的 `[peers]` 範例加入 `# encrypt = true`。

  驗證：人工對照 delta spec；`emfor check-config` 讀取 `examples/emfor.toml` 與取消 `[peers]` 註解的版本都通過。
- [x] 4.2 [after: 4.1] 在 `CHANGELOG.md` 未發布區段加入條目：peer 訊息預設加密（`EMP2`）；升級新舊並存的 peer group 時先設 `encrypt = false` 的步驟；沒有前向保密。驗證：人工檢查條目與 proposal 的 What Changes 相符。

## 5. 整體檢查

- [x] 5.1 [after: 2.1, 3.1, 4.2] 依 RELEASING.md，在 macOS 與 Debian 容器跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-check -p emfor-daemon --target-dir target/fuzzcheck -- -D warnings` 與 `cargo test --workspace --all-features`，並以 `-max_total_time=60` 跑 `peers` 與 `config` fuzz target。驗證：所有指令 0 個錯誤、0 個失敗，`fuzz/artifacts/` 沒有新檔。

## 實作備註

- 1.2 的已知答案改由 `crates/emfor-daemon/tests/emp2_kat.py`（純 Python 的 RFC 5869／RFC 8439 實作，先對照 RFC 測試向量）計算，不是 Python `cryptography`；測試註解指向這個腳本。見 design.md「已知答案測試」。
- 2.1 的「兩個 daemon 都 `encrypt = true` 時互相在線」由 `tests/peers.rs` 的 `encrypting_members_are_live` 檢查（兩邊在線、`malformed` 與 `auth` 都是 0、沒有明文警告）；「datagram 以 `EMP2` 開頭、明文中找不到 `site`」由 `heartbeats_are_encrypted_by_default` 檢查。
