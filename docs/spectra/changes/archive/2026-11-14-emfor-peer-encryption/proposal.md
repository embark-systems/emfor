## Why

peer group 的訊息（`heartbeat`、`down`、`result`）目前是 `EMP1` 格式：以共享 secret 的 HMAC-SHA256 驗證，防竄改與重放，但 payload 是明文 JSON。任何能看到這段網路流量的人都能讀到 site、成員位址、服務的 tag、backend 位址與檢查結果，等於拿到內部服務的拓撲與即時健康狀態。

## What Changes

- 新的訊息格式 `EMP2`：`EMP2`、12 bytes 隨機 nonce，接著是以 ChaCha20-Poly1305 加密的 payload 與 16 bytes 驗證標記。加密金鑰以 HKDF-SHA256 從 `secret_file` 的 secret 導出，`EMP2` 本身作為附加驗證資料。加密同時取代 HMAC，所以竄改照樣被擋下。
- payload 的 JSON 欄位、`boot`／`seq`／`ts` 的重放檢查、1200 bytes 上限、heartbeat 的 `off` 最多 80 個 tag、secret 輪替（換 secret 後 60 秒才改用新的，舊的 120 秒內仍接受）都不變。
- `[peers]` 新增 `encrypt`（boolean，預設 `true`）：`true` 時送出 `EMP2`，`false` 時送出 `EMP1`。不論設定為何，都接受 `EMP1` 與 `EMP2`，讓 peer group 可以逐台升級。`encrypt = true` 卻收到 `EMP1` 時，每個來源每 60 秒記一行 `warn`，提醒還有成員送明文。`encrypt` 可以在 reload 時修改。
- **升級注意**：舊版 daemon 不認得 `EMP2`，會把它當成 `malformed` 丟掉。peer group 裡新舊版本並存時，要先在新版設 `encrypt = false`，全部升級後再改為 `true` 並 reload。手冊與 CHANGELOG 說明這個步驟。
- daemon 新增 RustCrypto 的 `chacha20poly1305` 與 `hkdf`，和現有的 `hmac`、`sha2` 同一家族。
- 手冊第 3 章（`encrypt`、secret 的用途）、第 8 章（`auth` 的意義）、`docs/deploy/daemon.md`、`fuzz/fuzz_targets/peers.rs` 與 CHANGELOG 跟著更新。

## Non-Goals

- 改用 QUIC 或 DTLS：需要憑證、連線狀態與重新握手，換來的可靠傳輸對每秒重送一次的 heartbeat 用處不大。
- 前向保密（forward secrecy）：secret 外洩時，錄下的舊流量可被解開。要做到需要金鑰交換，不在本次範圍；手冊建議定期更換 secret。
- 隱藏訊息長度與送出時間：仍看得到每秒一次、大小相近的封包。
- 移除 `EMP1`：本次保留，讓升級可以分批進行。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: 修改「Peer group configuration」（新增 `encrypt`）、「Peer message authentication and replay protection」（`EMP2` 格式與兩種格式的接受規則）、「Peer listen requires restart」（`encrypt` 可在 reload 時修改）。

## Impact

- 受影響的 spec：`peer-group`
- 受影響的程式碼：
  - crates/emfor-daemon/src/peers.rs
  - crates/emfor-daemon/src/config.rs
  - crates/emfor-daemon/Cargo.toml
  - Cargo.lock
  - crates/emfor-daemon/tests/peers.rs
  - fuzz/fuzz_targets/peers.rs
  - docs/manual/03-configuration.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/08-operations.md
  - docs/manual.zh_TW/08-operations.md
  - docs/deploy/daemon.md
  - examples/emfor.toml
  - CHANGELOG.md
- store 格式、client、metrics 名稱不變。
