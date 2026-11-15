## Why

peer group 的加密訊息 `EMP2` 用 ChaCha20-Poly1305，每則訊息的 96-bit nonce 從 OS 亂數產生。同一把金鑰下，隨機 96-bit nonce 的慣用上限是 2^32 則訊息（此時兩則訊息 nonce 相同的機率約 2^-33）；nonce 一旦重複，攻擊者可以得到兩則明文的 XOR，並偽造 Poly1305 標記，等於加密與驗證同時失效。

2026-11-14 的 review 估計大型 peer group 每秒約 7500 則訊息（估計值，未量測；可在實際群組上以 `emfor_peer_notifications_total` 與 `emfor_peer_results_total` 加上每秒的 heartbeat 數確認），不到一週就會超過 2^32。`[peers] secret_file` 通常不會那麼常換，所以這個上限在大型群組裡是真的會碰到的。

XChaCha20-Poly1305 用 192-bit nonce，隨機產生時碰撞機率在任何實際訊息量下都可以忽略，而且和現在一樣不需要跨重啟保存任何狀態。

## What Changes

- 新的 datagram 格式 `EMP3`：`EMP3` 四個 bytes、24 bytes 的 nonce（每則訊息由 OS 亂數產生）、以 XChaCha20-Poly1305 加密的 payload 與 16 bytes 標記；AAD 為 `EMP3`；金鑰為 HKDF-SHA256（salt `emfor peers`、ikm 為 secret、info `EMP3 xchacha20-poly1305`）的 32 bytes。長度 46 到 1200 bytes；reason 照現有規則截短，讓 datagram 不超過 1200 bytes。
- 接收端一律接受 `EMP1`（仍受 `accept_plaintext` 限制）、`EMP2` 與 `EMP3`。
- 依成員各自選擇送出的加密格式（`encrypt = true` 時）：這個版本送出的 `EMP2` 都在 payload 帶 `"emp3": true`（0.4.0 會忽略）。對每個成員先送 `EMP2`；一旦接受了該成員送來的 `EMP3`，或帶 `"emp3": true` 的 `EMP2`，就改送 `EMP3` 給它；之後若接受到它送來不帶這個欄位的 `EMP2`，或 `EMP1`（例如降回 0.4.0），就退回 `EMP2`。狀態只存在記憶體，daemon 重啟後每個成員都從 `EMP2` 開始；只有通過驗證的訊息會改變它。格式改變時記一行 `info` log。
- 同一則訊息（同一個 `seq`）對每種需要的格式最多加密一次，再分別送給對應的成員。
- `encrypt = false` 照舊對所有成員送 `EMP1`。
- 不需要新增設定，0.4.0 的群組可以滾動升級而不中斷。
- 修正 spec 中 `accept_plaintext = false` 那句把 `EMP1` 誤寫成 `EMP2` 的筆誤。
- 文件：手冊（en、zh_TW）`[peers] encrypt` 與升級說明、CHANGELOG。

## Non-Goals (optional)

- forward secrecy（每次連線換金鑰）。
- 停止接受 `EMP2`（之後可另加類似 `accept_plaintext` 的選項）。
- 改變 `EMP1`。
- 用設定指定送出格式：使用者已選定依成員自動選擇。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: "Peer message authentication and replay protection" 增加 `EMP3` 格式、長度邊界與依成員選擇格式的規則。

## Impact

- Affected specs: peer-group
- Affected code:
  - Modified: crates/emfor-daemon/src/peers/wire.rs, crates/emfor-daemon/src/peers/crypto.rs, crates/emfor-daemon/src/peers/group.rs, crates/emfor-daemon/src/peers/task.rs, crates/emfor-daemon/tests/emp2_kat.py, crates/emfor-daemon/tests/peers.rs, docs/manual/03-configuration.md, docs/manual.zh_TW/03-configuration.md, CHANGELOG.md
  - New: none
  - Removed: none
- Dependencies: 沿用 chacha20poly1305 0.11.0（已內建 `XChaCha20Poly1305`），不新增 crate。
- 線上格式：新增 `EMP3`；0.4.0 成員收到 `EMP3` 會計為 `malformed`，但依成員選擇格式保證不會對它們送 `EMP3`。
