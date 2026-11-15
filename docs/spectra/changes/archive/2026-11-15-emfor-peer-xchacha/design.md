## Context

peer 訊息現在有兩種格式（`crates/emfor-daemon/src/peers/wire.rs`、`crypto.rs`）：`EMP1` 以 HMAC-SHA256 簽章的明文，以及 `EMP2` 以 ChaCha20-Poly1305 加密、12 bytes 隨機 nonce。`Group::encode`（`group.rs`）依 `encrypt` 產生一份 datagram，`task.rs` 以 `send_all` 把同一份 bytes 送給所有其他成員。`Group::check` 依前四個 bytes 分辨格式並回傳 `Format::Plain` 或 `Format::Encrypted`。0.4.0 已發佈 `EMP2`，0.4.0 的成員收到不認得的開頭會計為 `malformed`。

依賴 chacha20poly1305 0.11.0 已內建 `XChaCha20Poly1305` 與 `XNonce`（預設 feature 即可），不新增 crate。

## Goals / Non-Goals

**Goals:**

- 新增 `EMP3`（XChaCha20-Poly1305、24 bytes 隨機 nonce），解除 96-bit 隨機 nonce 在同一把金鑰下約 2^32 則訊息的上限。
- 含 0.4.0 成員的群組可以滾動升級，不需改設定、不會斷訊。
- 不保存任何跨重啟的狀態。

**Non-Goals:**

- forward secrecy、停止接受 `EMP2` 的選項、改變 `EMP1`、以設定指定送出格式。

## Decisions

### EMP3 datagram format

`EMP3` ‖ nonce（24 bytes，每則訊息由 OS 亂數產生）‖ XChaCha20-Poly1305 密文 ‖ 16 bytes 標記；AAD 為 `EMP3` 四個 bytes。長度下限 46 bytes（4 + 24 + 16 + 至少 2 bytes 的 payload，比照 `EMP2` 的 34），上限沿用 `MAX_DATAGRAM` 1200 bytes。`payload_for` 以 `EMP3` 的 overhead（44 bytes）截短 reason。

替代方案：在 `EMP2` 之上改用計數器 nonce，或依 `boot` 分金鑰。前者需要保證跨重啟不重複，0.4.0 的設計已因此不採用；後者也要改線上格式，卻比 XChaCha20 複雜。不採用。

### EMP3 key derived separately from EMP2

金鑰為 HKDF-SHA256（salt `emfor peers`、ikm 為 secret、info `EMP3 xchacha20-poly1305`）的 32 bytes，和 `EMP2` 的金鑰（info `EMP2 chacha20-poly1305`）不同，讓兩種演算法永遠不共用同一把金鑰。`Secret::new` 同時導出兩把金鑰；`Keys` 的目前與前一個 secret 都同時有兩把，secret rotation 的規則不變。

### Per-member encrypted format

`Group` 為每個成員記一個「送出的加密格式」，初始為 `EMP2`。每當接受（通過所有檢查）一則來自該成員的 `EMP2` 或 `EMP3` 訊息就重新決定：`EMP3` 訊息，或 payload 帶 `"emp3": true` 的 `EMP2` 訊息，設為 `EMP3`；不帶這個欄位的 `EMP2` 訊息（0.4.0 送的）設為 `EMP2`；接受 `EMP1` 也設為 `EMP2`（見 "Plaintext from a member resets it to EMP2"）。值改變時記一行 `info` log，寫出成員與新格式。這個狀態只存在記憶體；daemon 重啟、或成員從 `members` 移除後再加回，都回到 `EMP2`。

只有通過驗證的訊息會改變狀態，`emp3` 欄位也在加密的 payload 裡，所以沒有 secret 的攻擊者最多只能擋掉訊息，讓雙方停在 `EMP2`，也就是 0.4.0 的狀態；重放舊的 `EMP2` 訊息會先被 replay 檢查擋下。

替代方案：以設定指定格式（例如暫時送 `EMP2` 的選項），或只送 `EMP3`。使用者選定依成員自動選擇。

### EMP2 messages announce EMP3

Supersedes 上一版的「接受該成員的 `EMP3` 才改送 `EMP3`」：實作時 `encrypting_members_are_live` 顯示兩個新版成員都等對方先送 `EMP3`，永遠停在 `EMP2`。改為這個版本送出的每則 `EMP2` 訊息（heartbeat、`down`、`result` 都算）的 payload 帶 `"emp3": true`，0.4.0 會忽略這個未知欄位。只放在 heartbeat 不夠：規則看最近一則加密訊息，一則不帶欄位的 `down` 會讓對方誤退回 `EMP2`。接收端只把 JSON 的 `true` 當作有這個欄位，其他值當作沒有；`EMP1` 與 `EMP3` 訊息不寫這個欄位，收到也忽略。

`Message` 增加 `emp3: bool`；`payload_for` 只在它為 true 時寫出欄位；`Group::encode` 只在 `EMP2` 那批設為 true。欄位為 `,"emp3":true` 12 bytes：最大的 `EMP2` heartbeat 由估算的 1160 bytes 增為約 1172 bytes（估算值，由 `largest_heartbeat_fits_one_datagram` 確認）。

替代方案：新成員先試送 `EMP3`、沒回應再退回。0.4.0 會把它計為 `malformed` 並記 `warn`，UDP 也沒有「沒回應」可判斷。不採用。

### Plaintext from a member resets it to EMP2

Supersedes 上一版的「接受 `EMP1` 不改變它」：review 發現成員以 `encrypt = false`（0.3.0 升級說明留下的設定）降回 0.4.0 時，只送 `EMP1`，對方照舊送 `EMP3`，0.4.0 全部計為 `malformed`，直到對方重啟。改為接受 `EMP1` 時設為 `EMP2`：新版與 0.4.0 都收 `EMP2`，所以對任何成員都安全；`EMP1` 也要通過 HMAC 驗證，沒有 secret 無法用它降級。以 `encrypt = false` 執行這個版本的成員因此一直收到 `EMP2`，和 0.4.0 相同。

降級後，對方在接受那個成員的第一則訊息之前送出的 `EMP3` 仍會被丟掉；heartbeat 每秒一則，約 1 秒。手冊寫明這一點。

### Encode each needed format once per message

`Group::encode` 改為回傳一組（datagram、收件成員）：`encrypt = true` 時，依成員目前的格式分成 `EMP2` 與 `EMP3` 兩批，每批只加密一次，兩份 datagram 帶同一個 `seq`；只需要其中一種時就只加密一次。`encrypt = false` 時照舊只產生一份 `EMP1` 給所有成員。`task.rs` 的兩處 `send_all` 改為逐批送出，送出計數照舊以成員數計。

### Independent known-answer test for EMP3

`crates/emfor-daemon/tests/emp2_kat.py` 增加 HChaCha20 與 XChaCha20-Poly1305 的純 Python 實作，執行時先對照 draft-irtf-cfrg-xchacha-03 的 HChaCha20（2.2.1）與 AEAD_XChaCha20_Poly1305（A.3.1）測試向量，再算出固定 secret、nonce 與 payload 的 `EMP3` bytes；`crypto.rs` 的已知答案測試比對這份 bytes。

## Implementation Contract

**Observable behavior**

- `encrypt = true` 的 daemon 送出的 `EMP2` 訊息都帶 `"emp3": true`。對一個還沒有接受過其加密訊息的成員送 `EMP2`；接受該成員一則 `EMP3`，或帶 `"emp3": true` 的 `EMP2` 之後，送給它的下一則訊息以 `EMP3` 開頭；之後接受它一則不帶這個欄位的 `EMP2`，再下一則又是 `EMP2`。每次改變記一行 `info` log。兩個這個版本的 daemon 互為成員時，收到對方第一則訊息後互送 `EMP3`。
- 接收端接受 46 到 1200 bytes、以 `EMP3` 開頭且以目前或仍被接受的前一個 secret 解得開的訊息；45 或 1201 bytes 為 `malformed`，竄改任何 byte（含 nonce 與 AAD）為 `auth`。其餘接受檢查（JSON、sender、site、stale、replay）與 `EMP2` 相同。
- `accept_plaintext = false` 只拒絕 `EMP1`，`EMP2` 與 `EMP3` 照常接受。
- 最大的 heartbeat（80 個 tag、最長的 `from` 與 `site`、`seq` 與 `ts` 為 u64 上限）以 `EMP3` 送出時不超過 1200 bytes（以 Python 估算為 1172 bytes，由測試確認）。
- 指標名稱與 reason 不變；`EMP3` 解不開計為 `auth`，與 `EMP2` 相同。

**Acceptance criteria**

- `crypto.rs` 與 `wire.rs` 的單元測試涵蓋 `EMP3` 往返、竄改、長度邊界 45／46／1200／1201、前一個 secret 30／90 秒接受與 130 秒拒絕、已知答案測試、最大 heartbeat。
- `group.rs` 的單元測試涵蓋依成員選擇格式的轉換（`EMP2` → `EMP3` → `EMP2`，含帶與不帶 `emp3` 的 `EMP2`、值不是 `true` 的 `emp3`）、`EMP1` 設為 `EMP2`、一則訊息對兩種格式各加密一次且 `seq` 相同、`EMP2` 那批帶 `emp3` 而 `EMP3` 與 `EMP1` 不帶。
- `crates/emfor-daemon/tests/peers.rs` 的多 daemon 整合測試在兩個新版成員之間最後收發 `EMP3`，並全部通過。
- Debian 映像 `emfor-clients-test` 的完整檢查通過（fmt、clippy 含 `--cfg fuzzing`、`cargo test --workspace --all-features`、`make -C clients test`）。

**In scope**：`EMP3` 格式與金鑰、接收、依成員選擇格式、分批 encode、文件、CHANGELOG、peer-group spec。

**Out of scope**：forward secrecy、停止接受 `EMP2`、改變 `EMP1`、新的設定鍵、新的指標。

## Risks / Trade-offs

- [成員之間短暫格式不一致：A 已改送 `EMP3`，B 在同一時間降回 0.4.0] → B 會把 `EMP3` 計為 `malformed`，直到 A 接受 B 的下一則 `EMP2`（heartbeat 每秒一則，所以約 1 秒內恢復）。
- [一則訊息加密兩次的成本] → 只在群組同時有新舊成員時發生，payload 至多 1200 bytes，成本可忽略。
- [沒有通過驗證就無法升到 `EMP3`：例如 secret 不一致] → 那時本來就互不接受訊息，和現在相同。
- [Python 估算的最大 heartbeat 若和實際不符] → `largest_heartbeat_fits_one_datagram` 以 `EMP3` 確認；超過 1200 bytes 時停下來回報，不自行調低 80 個 tag 的上限。

## Migration Plan

升級時不需改設定：先升級的成員對尚未升級的成員繼續送 `EMP2`（0.4.0 忽略 `emp3` 欄位），兩個都升級的成員在收到對方第一則訊息（帶 `"emp3": true` 的 `EMP2`）後改用 `EMP3`。回退到 0.4.0 時，其他成員在接受到它的 `EMP2`（或 `encrypt = false` 時的 `EMP1`）後自動退回，期間約 1 秒的訊息會被它丟棄。手冊的升級說明改寫為這個流程。

## Open Questions

（無）
