## 1. EMP3 datagram format

- [x] 1.1 實作 EMP3 datagram format 與 EMP3 key derived separately from EMP2：`wire.rs` 新增 `MAGIC_V3`、24 bytes nonce、44 bytes overhead、下限 46 bytes；`Secret::new` 另導出 info `EMP3 xchacha20-poly1305` 的金鑰；`crypto.rs` 新增以 XChaCha20-Poly1305 加密的 `EMP3` 封裝與解密（AAD `EMP3`），`Keys` 以目前與前一個 secret 解 `EMP3`，`open_with` 接受 `EMP3`；`payload_for` 以 `EMP3` 的 overhead 截短 reason。驗證：`crypto.rs`、`wire.rs` 的新單元測試涵蓋 `EMP3` 往返、竄改任一 byte（含 nonce 與 AAD）解不開、長度 45／46／1200／1201、前一個 secret 30／90 秒可解與 130 秒不可解、reason 最長的訊息與 `largest_heartbeat_fits_one_datagram` 以 `EMP3` 不超過 1200 bytes，先確認新測試在實作前失敗。
- [x] 1.2 [after: 1.1] Independent known-answer test for EMP3：`crates/emfor-daemon/tests/emp2_kat.py` 增加純 Python 的 HChaCha20 與 XChaCha20-Poly1305，執行時先對照 draft-irtf-cfrg-xchacha-03 第 2.2.1 節與附錄 A.3.1 的測試向量，再算出固定 secret、nonce 與 payload 的 `EMP3` bytes；`crypto.rs` 新增已知答案測試比對這份 bytes。驗證：`python3 crates/emfor-daemon/tests/emp2_kat.py` 印出兩份測試向量通過與 `EMP3` 的 hex，新的已知答案測試通過。
- [x] 1.3 [after: 1.1] `emfor_daemon::peers::fuzz_decode` 增加第四種模式，以固定 nonce 封裝 `EMP3` 後解密並解析 payload，讓 `fuzz/fuzz_targets/peers.rs` 也能走到 `EMP3` 的解密路徑。驗證：`RUSTFLAGS="--cfg fuzzing" cargo clippy -p emfor-daemon -- -D warnings` 通過，`fuzz` 目錄 `cargo fmt --check` 通過，並在 fuzz target 跑 60 秒沒有失敗。

## 2. Per-member encrypted format

- [x] 2.1 [after: 1.1] 依 spec "Peer message authentication and replay protection"，`Group::check` 接受 `EMP3`（回傳新的 `Format` 變體），`accept_plaintext = false` 只拒絕 `EMP1`；實作 Per-member encrypted format：每個成員一個送出格式，初始 `EMP2`，接受該成員的 `EMP2` 或 `EMP3` 訊息時設為該格式，接受 `EMP1` 或丟棄訊息時不變，`set_members` 加入的新成員為 `EMP2`，值改變時記一行 `info` log。驗證：`group.rs` 的新單元測試涵蓋 `EMP2` → `EMP3` → `EMP2` 三個轉換、`EMP1` 與被拒絕的 `EMP3` 不改變狀態、`accept_plaintext = false` 接受 `EMP3`、spec 範例表中所有 `EMP3` 列的結果。
- [x] 2.2 [after: 2.1] Encode each needed format once per message：`Group::encode` 回傳（datagram、收件成員）的批次，`encrypt = true` 時依成員格式分成 `EMP2` 與 `EMP3`，每批只加密一次且 `seq` 相同，`encrypt = false` 時一份 `EMP1` 給所有成員；`task.rs` 送 heartbeat、`down`、`result` 的兩處改為逐批送出，送出計數仍以成員數計。驗證：`group.rs` 新單元測試確認兩種格式的成員各收到一份、`seq` 相同、只需一種格式時只加密一次；`crates/emfor-daemon/tests/peers.rs` 現有測試（含 `heartbeats_are_encrypted_by_default`、`encrypting_members_are_live`、`encrypt_follows_reloads`）全部通過。
- [x] 2.3 [after: 2.4] 整合測試：`crates/emfor-daemon/tests/peers.rs` 新增一個測試，以測試端的 UDP socket 扮演成員，先確認 daemon 送來 `EMP2`，再以該成員的地址送一則有效的 `EMP3` heartbeat（以 crate 公開的封裝函式產生），確認 daemon 之後送來 `EMP3`、日誌有一行 `info` 寫出該成員與 `EMP3`；再送一則不帶 `emp3` 的有效 `EMP2` heartbeat，確認之後又是 `EMP2`；兩個真的 daemon 互為成員時兩邊都 live、沒有 `malformed` 或 `auth`。驗證：`cargo test -p emfor-daemon --test peers` 通過，並先確認新測試在 2.1／2.2 之前會失敗。

- [x] 2.4 [after: 2.2] 依 design "EMP2 messages announce EMP3"（取代 2.1 的「接受 `EMP3` 才改送」，因兩個新版成員會永遠停在 `EMP2`）：`Message` 增加 `emp3`，`payload_for` 只在為 true 時寫出 `"emp3":true`，`parse_payload` 只把 JSON `true` 當作有；`Group::encode` 的 `EMP2` 那批設為 true；`Group::accept` 接受 `EMP2` 時依 `emp3` 決定 `EMP3` 或 `EMP2`。驗證：`group.rs` 單元測試涵蓋帶與不帶 `emp3` 的 `EMP2` 轉換、`"emp3":"yes"` 視為沒有、`EMP2` 那批帶欄位而 `EMP3`、`EMP1` 不帶；`largest_heartbeat_fits_one_datagram` 以帶欄位的 `EMP2` 不超過 1200 bytes；`encrypting_members_are_live` 兩邊都記下改送 `EMP3` 的 `info`。

- [x] 2.5 [after: 2.4] 依 design "Plaintext from a member resets it to EMP2"（取代「接受 `EMP1` 不改變」）：`Group::accept` 接受 `EMP1` 時把該成員設為 `EMP2`，改變時照樣記 `info`。驗證：`group.rs` 單元測試 `format_follows_the_member` 與 `emp2_with_emp3_field_moves_the_member_to_emp3` 改為 `EMP1` 讓 `EMP3` 退回 `EMP2`，新增情境 "Member rolled back with encryption off" 的單元測試；手冊（en、zh_TW）與 CHANGELOG 的降級說明寫明 `EMP1` 也會退回與約 1 秒的訊息遺失；`cargo test -p emfor-daemon --lib peers::` 與 `--test peers` 通過、clippy 無警告。

## 3. 文件

- [x] 3.1 [after: 2.4] 手冊 `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md` 的 `[peers] encrypt` 說明改寫為依成員選擇 `EMP2` 或 `EMP3`，升級段落說明含 0.4.0 成員的群組不需改設定即可滾動升級；`CHANGELOG.md` 的 Unreleased 新增一條說明 `EMP3`、原因（96-bit 隨機 nonce 的 2^32 則上限）與升級方式。驗證：人工比對文件與 spec 的訊息格式、長度與轉換規則一致，`grep -n "EMP3"` 在三個檔案都找得到。

## 4. 檢查

- [x] 4.1 [after: 1.2, 1.3, 2.3, 3.1] 在 Debian 映像 `emfor-clients-test` 跑完整檢查：`cargo fmt --all --check`（含 `fuzz/`）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`--cfg fuzzing` 的 clippy、`cargo test --workspace --all-features`、`make -C clients test`。驗證：全部通過，記下通過的測試數。
