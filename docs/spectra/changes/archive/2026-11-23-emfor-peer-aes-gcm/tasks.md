## 1. EMP4 格式與金鑰

- [x] 1.1 實作 requirement「Peer message authentication and replay protection」的 `EMP4` datagram 與金鑰（design：「EMP4 的 nonce 是計數器，金鑰依 boot 分開」、「實作用 aws-lc-rs，包在 crypto.rs 裡」）。`crates/emfor-daemon/Cargo.toml` 新增直接依賴 `aws-lc-rs`（版本與 `Cargo.lock` 現有的 1.19.0 一致，不新增 crate）；`wire.rs` 新增 `MAGIC_V4`、`OVERHEAD_V4`（36）、`MIN_DATAGRAM_V4`（38）；`crypto.rs` 新增以 `aws_lc_rs::hkdf` 導出 (secret, `boot`) 的金鑰、`encode_encrypted_v4`（`EMP4` ‖ `boot` ‖ 計數器 ‖ 密文 ‖ 標記，AAD 為前 20 bytes，nonce 為 4 個 0 加計數器）與 `Keys::open_v4`（對每個仍被接受的 secret 各試一次，`open_with` 也認得 `EMP4`），不改 `EMP1`、`EMP2`、`EMP3`。驗證：`crypto.rs` 單元測試——往返（每種 kind，payload 不含明文 `site`）、改動 `boot`／計數器／密文／標記任何一個 byte 得 `auth`、長度 37／38／1200／1201、前一個 secret 30／90 秒接受與 130 秒拒絕、同一送出者連續 datagram 的 (金鑰, nonce) 不重複、不同 `boot` 得到不同金鑰、最大 heartbeat 以 `EMP4` 送出不超過 1200 bytes；測試帶 `// spectra-scenario: peer-group / Peer message authentication and replay protection / acceptance checks` 註解；`tools/docker-test.sh` 通過。
- [x] 1.2 [after: 1.1] 已知答案測試。`crates/emfor-daemon/tests/emp2_kat.py` 增加獨立的 AES-256-GCM 與 HKDF-SHA256 實作（Python 標準庫；先對照 NIST 的 AES-GCM 測試向量與 RFC 5869 的 HKDF 測試向量，通過才輸出），算出固定 secret、`boot`、計數器與 payload 的 `EMP4` bytes；`crypto.rs` 的已知答案測試比對這份 bytes。驗證：該測試與 Python 腳本自檢都通過，`tools/docker-test.sh` 通過。

## 2. cipher 設定

- [x] 2.1 實作 requirement「Peer group configuration」的 `cipher`（design：「`cipher` 是明確的設定，不自動協商」）。`config.rs` 新增 `Cipher` enum 與 `PeersConfig.cipher`（預設 `chacha20-poly1305`），驗證：不是字串或不是兩個值之一為 `peers.cipher` 錯誤、`encrypt = false` 加 `aes-256-gcm` 為 `peers.cipher` 錯誤，錯誤訊息照 spec 範例表。驗證：`config.rs` 單元測試涵蓋範例表「peer config validation」新增的每一列，帶 `// spectra-scenario: peer-group / Peer group configuration` 註解；`emfor check-config` 對 `cipher = "aes-256-gcm"` 的設定檔結束代碼 0；`tools/docker-test.sh` 通過。

## 3. 送出與接收

- [x] 3.1 [after: 1.1, 2.1] 把 `EMP4` 接進 `Group` 與 task（design：「`cipher` 是明確的設定」）。`group.rs` 新增 `Format::EncryptedV4`、`Group::set_cipher`、送出端計數器（每份 `EMP4` datagram 加 1，每次啟動從 1）；`Group::encode` 在 `cipher = aes-256-gcm` 且 `encrypt` 時對每個成員送一份 `EMP4`（同一份 datagram），其餘情況與現在相同；`Group::check` 依前四個 bytes 分派 `EMP4`、檢查長度 38 到 1200 與標頭 `boot` 等於 payload 的 `boot`（否則 `malformed`）；接受 `EMP4` 不改變 `EMP2`／`EMP3` 的選擇、不算明文，`accept_plaintext = false` 也接受；`task.rs` 在啟動與 reload 時與 `set_encrypt` 一起套用 `cipher`。驗證：`group.rs` 單元測試涵蓋 spec 的情境 "Members send AES-GCM"、"AES-GCM key and counter after a restart"、"Cipher changed by a reload" 與範例表新增的 `EMP4` 列（含標頭與 payload `boot` 不同為 `malformed`、`EMP5` 開頭為 `malformed`、`accept_plaintext = false` 接受 `EMP4`、`EMP4` 不改變送給成員的 `EMP2`／`EMP3`），測試帶對應的 `// spectra-scenario: peer-group / ...` 註解；情境 "Earlier version receives AES-GCM" 需要舊版 binary，不在此 repo 自動測試，改由手冊的升級順序（任務 5.1）與下一個任務的整合測試涵蓋「預設值不送 `EMP4`」；`tools/docker-test.sh` 通過。
- [x] 3.2 [after: 3.1] 多 daemon 整合測試。`crates/emfor-daemon/tests/peers.rs` 新增：兩個 daemon 都設 `cipher = "aes-256-gcm"`，互相 live，並從網路上抓到的 datagram 以 `EMP4` 開頭且計數器遞增；其中一個 reload 改回預設後兩者仍 live、之後的 datagram 不是 `EMP4`。驗證：該測試在 `tools/docker-test.sh` 內通過，帶 `// spectra-scenario: peer-group / Cipher changed by a reload` 註解。

## 4. 稽核 log

- [x] 4.1 [after: 2.1] 實作 requirement「Configuration audit log」的 `cipher` 欄位。`audit.rs` 的 `peer_changes` 在 `accept_plaintext` 之後比較 `cipher`，`field=cipher`、`applies=now`，沒設定時寫 `chacha20-poly1305`。驗證：`audit/tests.rs` 單元測試——`cipher` 由未設改為 `aes-256-gcm` 得一行 `peers_changed`（`from=chacha20-poly1305`、`to=aes-256-gcm`、`applies=now`），兩邊相同得空，帶 `// spectra-scenario: service-config / Configuration audit log / Cipher changed` 註解；`tools/docker-test.sh` 通過。

## 5. 文件

- [x] 5.1 [after: 3.1, 4.1] 手冊與 CHANGELOG（design：「手冊寫清楚 FIPS 的限制」）。`docs/manual/11-peer-groups.md`（格式表加 `EMP4`、`cipher` 選項、升級順序與復原方法）、`13-security.md`（新增「FIPS」一節：沒有 FIPS 模式、各用途的演算法與實作、ChaCha20-Poly1305 不在核准清單、aws-lc-rs 預設版不是驗證過的模組且 `ring` 仍在、MD5 的用途、主機開 FIPS 模式時的行為；並更新稽核欄位表若有）、`18-config-reference.md`（`cipher`）、zh_TW 對應檔，與 `CHANGELOG.md` 的 Unreleased 一條。驗證：`python3 tools/check-manual-links.py` 回報 0 problems，中英內容與 spec 用語（`cipher` 值、`EMP4`、升級順序）一致，並讀過 FIPS 一節確認沒有任何「符合 FIPS」的字句。

## 6. 檢查

- [x] 6.1 [after: 3.2, 5.1] 全部檢查通過：`tools/docker-test.sh`（fmt、clippy -D warnings、workspace 測試、doc tests）通過，`spectra analyze emfor-peer-aes-gcm` 與 `spectra validate emfor-peer-aes-gcm` 沒有 Critical 或失敗。
