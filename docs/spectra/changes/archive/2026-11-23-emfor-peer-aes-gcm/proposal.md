## Why

要求 FIPS 的環境（電信、政府）需要核准演算法：聯防加密現在只有 ChaCha20-Poly1305（EMP2、EMP3），不在 FIPS 核准清單內。加入 AES-256-GCM，並用 TLS 已經連結的 aws-lc-rs 實作，讓之後要走 FIPS 版時只剩換 build feature 這一步。這不等於 emfor 符合 FIPS：TLS 以外仍有 ring，`radius` 外掛用 MD5，這一點要寫進手冊。

## What Changes

- 聯防訊息新增第四種格式 `EMP4`：AES-256-GCM，實作用 `aws-lc-rs`，金鑰以 HKDF-SHA256 從 secret 與送出者的 `boot` 導出，nonce 為每把金鑰內遞增的計數器，同一把金鑰下不會重複。
- `[peers]` 新增選用的 `cipher`：`"chacha20-poly1305"`（預設，行為與現在完全相同，EMP2 與 EMP3 依成員自動選擇）或 `"aes-256-gcm"`（送 `EMP4` 給所有成員）。需要 `encrypt = true`；reload 後立即生效。
- 接收端不論 `cipher`、`encrypt` 為何，都接受 `EMP4`（與 EMP2、EMP3 相同）。舊版成員把 `EMP4` 當成不認得的開頭，計為 `malformed`，所以升級要先全部成員升版、再改 `cipher`。
- reload 稽核 log 的 `peers_changed` 認得 `cipher`。
- 手冊：第 11 章（格式表、`cipher`、升級順序）、第 13 章新增「FIPS」一節（emfor 沒有 FIPS 模式、用到的演算法與實作、哪些不在驗證模組內）、第 18 章設定參考、zh_TW 對應與 CHANGELOG。
- 新增直接依賴 `aws-lc-rs`（已在 Cargo.lock，由 rustls 帶入）。

## Non-Goals

- FIPS 版 build feature（`aws-lc-fips-sys`）、移除 `ring`、`radius` 外掛的 MD5、用 aws-lc-rs 取代 HMAC/SHA-2 等其他用途；做完之後不宣稱 emfor 符合 FIPS。
- 依成員自動協商 `EMP4`（沿用 `emp3` 欄位那種做法）：升級順序由手冊說明，`cipher` 由維運明確設定。
- 停止接受 `EMP2`、`EMP3`，或改變 `EMP1`。
- client 端（libemfor、各語言）變更：聯防只存在於 daemon。

## Capabilities

### New Capabilities

### Modified Capabilities

- `peer-group`: 「Peer group configuration」新增 `cipher` 鍵與驗證；「Peer message authentication and replay protection」新增 `EMP4` 格式、依 `cipher` 送出、接受規則與長度。
- `service-config`: 「Configuration audit log」的 `peers_changed` 欄位加入 `cipher`。

## Impact

- 程式：crates/emfor-daemon/Cargo.toml（新增 `aws-lc-rs`）、crates/emfor-daemon/src/peers/crypto.rs、wire.rs、group.rs、task.rs，crates/emfor-daemon/src/config.rs，crates/emfor-daemon/src/audit.rs 與 cli.rs（reload 時套用 `cipher`），crates/emfor-daemon/tests/（已知答案測試向量與整合測試）。
- 文件：docs/manual/11-peer-groups.md、13-security.md、18-config-reference.md 與 docs/manual.zh_TW 對應檔，CHANGELOG.md，docs/spectra/specs/peer-group/spec.md 與 service-config/spec.md（經封存）。
- 相依：`aws-lc-rs` 已經由 rustls 連結進 daemon，不新增建置工具；`Cargo.lock` 的 aws-lc-rs 版本不變。
