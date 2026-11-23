## Context

聯防訊息現有三種格式（`crates/emfor-daemon/src/peers/wire.rs`、`crypto.rs`）：`EMP1` HMAC-SHA256 簽章、`EMP2` ChaCha20-Poly1305（12 bytes 隨機 nonce）、`EMP3` XChaCha20-Poly1305（24 bytes 隨機 nonce）。`EMP3` 當初就是為了避開 96-bit 隨機 nonce 在同一把金鑰下約 2^32 則訊息的上限（archive 2026-11-15-emfor-peer-xchacha）。`Group::encode`（`group.rs`）依 `encrypt` 與每個成員的「送出的加密格式」決定每個成員收到哪種 datagram；`task.rs` 在 reload 時用 `set_encrypt` 套用 `encrypt`。

AES-256-GCM 在 FIPS 核准清單內，ChaCha20-Poly1305 不在。`aws-lc-rs`（1.19.0）已經由 rustls 連結進 daemon，提供 AES-256-GCM 與 HKDF；之前選純 Rust 實作（archive 2026-11-14-emfor-peer-encryption）的理由是不想綁 C 實作，現在 TLS 已經綁了，這個理由不再成立。

## Goals / Non-Goals

**Goals:**

- 新增 `EMP4`（AES-256-GCM），以設定 `[peers] cipher = "aes-256-gcm"` 選用；預設行為完全不變。
- 同一把金鑰下 nonce 不會重複，且不靠隨機。
- 實作用 `aws-lc-rs`，讓之後換成 FIPS build 時，聯防不必再改格式。
- 手冊寫清楚：這不使 emfor 符合 FIPS。

**Non-Goals:**

- FIPS build feature（`aws-lc-fips-sys`）、移除 `ring`、`radius` 外掛的 MD5、其他用途的 HMAC/SHA-2 換成 aws-lc。
- 依成員自動協商 `EMP4`、停止接受舊格式、改變 `EMP1`、client 端變更。
- 新的指標或拒絕原因：`EMP4` 解不開計為 `auth`，長度不對計為 `malformed`，與 `EMP2`、`EMP3` 相同。

## Decisions

### EMP4 的 nonce 是計數器，金鑰依 boot 分開

`EMP4` datagram：`EMP4`（4 bytes）‖ `boot`（8 bytes，明文）‖ 計數器（8 bytes，big-endian，明文）‖ AES-256-GCM 密文 ‖ 16 bytes 標記。AAD 是前面這 20 bytes。nonce 是 4 個 0 bytes 加計數器。金鑰是 HKDF-SHA256（salt `emfor peers`、ikm 為 secret、info 為 `EMP4 aes-256-gcm` 加上 8 bytes 的 `boot`）的 32 bytes。

每個送出者在每次啟動時隨機選一個 `boot`（現有的做法），計數器從 1 開始，每送一份 `EMP4` datagram 加 1。所以「金鑰」是 (secret, boot) 一對，同一把金鑰下的 nonce 是這個送出者的計數器，不會重複；重啟會換 `boot`，也就換金鑰，不需要保存任何跨重啟的狀態。64-bit 計數器在每秒 1000 份的速度下要數十億年才會繞回。

替代方案：隨機 12 bytes nonce（和 `EMP2` 一樣）。所有成員共用一個 secret，NIST SP 800-38D 對隨機 nonce 的建議上限是同一把金鑰 2^32 次，成員多、訊息頻繁時幾個月到幾年就可能接近，`EMP3` 正是為了避開這個問題；不採用。替代方案：用現有訊息的 `seq` 當計數器；但 `seq` 在加密的 payload 裡，接收端要先解密才看得到，而且同一個 `seq` 可能被兩種格式各用一次，所以不用。

兩個送出者碰巧選到同一個 `boot`（64 bits 隨機）而共用金鑰的機率可以忽略，但它們各自的計數器若相同，就會重複 nonce；這個風險靠 `boot` 的隨機性接受，不另加成員位址進金鑰（`from` 在加密的 payload 裡，接收端解密前不知道）。明文的 `boot` 與計數器會讓旁觀者看到送出的份數，不洩漏內容。

### 實作用 aws-lc-rs，包在 crypto.rs 裡

`emfor-daemon` 直接依賴 `aws-lc-rs`（版本與 Cargo.lock 現有的 1.19.0 一致，預設 feature）。`crypto.rs` 用 `aws_lc_rs::hkdf` 導出金鑰、`aws_lc_rs::aead`（`AES_256_GCM`、`LessSafeKey`、`Aad`、`Nonce::assume_unique_for_key`）封包與解包。`EMP2`、`EMP3`、`EMP1` 的實作不動。`Secret` 增加第三把金鑰不可行（金鑰依 `boot` 變），改為在需要時以 `Secret::raw` 導出：送出者每次啟動導一次並保存，接收端依 `(secret, boot)` 導出，每個送出者只快取最近一個 `boot` 的金鑰。

接收端要在每個仍被接受的 secret（目前、前一個、最多兩個被取代的）各試一次，和 `EMP2`、`EMP3` 相同；導出一把金鑰是一次 HKDF，heartbeat 每成員每秒一份，成本可以忽略（尚未量測；若要確認，`cargo bench` 不是專案慣例，測試只確認結果正確）。

替代方案：RustCrypto 的 `aes-gcm`。純 Rust、不新增 C 相依，但永遠不會是驗證過的模組，和這個變更的目的不合；不採用（使用者在討論時選定 aws-lc-rs）。

### `cipher` 是明確的設定，不自動協商

`[peers] cipher` 為 `"chacha20-poly1305"`（預設）或 `"aes-256-gcm"`。預設時 `EMP2`／`EMP3` 的現有行為一個 byte 都不變。`"aes-256-gcm"` 時，`Group::encode` 對每個成員都送 `EMP4`，不看成員之前送過什麼。需要 `encrypt = true`，否則是設定錯誤。reload 套用方式和 `encrypt` 相同（`Group::set_cipher`，在 `task.rs` 與 `set_encrypt` 一起呼叫）；不需要重啟。

接收端不論自己的 `cipher`、`encrypt` 都接受 `EMP4`，`accept_plaintext = false` 也接受；接受 `EMP4` 不改變「送給該成員的 `EMP2`/`EMP3` 選擇」，也不當成明文。`MAGIC` 之外，只有 `Group::check` 的格式分派、長度範圍（最短 38 bytes：4+8+8+16 加 2 bytes 的 payload `{}`）與一個新的 `Format::EncryptedV4` 受影響。

替代方案：像 `emp3` 那樣在加密的 payload 裡帶 `emp4` 欄位讓成員自動協商。省去升級順序的限制，但要多一個欄位、多一組狀態轉換與測試，而使用者要的是「明確選用」；不採用。

### 稽核 log 加入 cipher

`audit.rs` 的 `peer_changes` 在 `encrypt`、`accept_plaintext` 之後加入 `cipher`（`applies=now`），沒設定的值寫 `chacha20-poly1305`。spec 的 service-config 稽核 requirement 同步更新。

### 手冊寫清楚 FIPS 的限制

第 13 章新增「FIPS」一節：emfor 沒有 FIPS 模式；列出每種用途的演算法與實作（TLS：aws-lc-rs 一般版；聯防：HMAC-SHA256／HKDF-SHA256／ChaCha20-Poly1305／XChaCha20-Poly1305 為 RustCrypto，AES-256-GCM 為 aws-lc-rs 一般版；MD5 用於負載分配與 `radius` 協定）；ChaCha20-Poly1305 不在核准清單；`aws-lc-rs` 預設版不是驗證過的模組，`ring` 仍在相依樹中；主機開 FIPS 模式時 emfor 不用作業系統的密碼庫，所以既不受影響、也不因此符合。

## Implementation Contract

**Observable behavior**

- `cipher` 沒設或為 `"chacha20-poly1305"` 時，所有 datagram 與升級前相同（`EMP1`／`EMP2`／`EMP3` 的選擇規則不變）。
- `cipher = "aes-256-gcm"` 且 `encrypt = true` 時，daemon 送給每個成員的每份 datagram 都以 `EMP4` 開頭，計數器從 1 起每份加 1，payload 不含明文的 `site`；重啟後 `boot` 與金鑰都換新、計數器回到 1。
- 接收端接受 38 到 1200 bytes、以 `EMP4` 開頭、標頭 `boot` 與 payload 的 `boot` 相同、且以目前或仍被接受的前一個 secret 解得開的訊息；37 或 1201 bytes 為 `malformed`，改動 `boot`、計數器、nonce 或密文的任何一個 byte 為 `auth`，標頭與 payload 的 `boot` 不同為 `malformed`；`EMP5` 開頭為 `malformed`。
- `cipher` 不是字串、值不是這兩個之一為 `peers.cipher` 錯誤；`encrypt = false` 加 `cipher = "aes-256-gcm"` 為 `peers.cipher` 錯誤。
- reload 把 `cipher` 改成 `"aes-256-gcm"` 或改回，下一份 datagram 就換格式，不用重啟；稽核 log 有 `change=peers_changed field=cipher applies=now`。
- 舊版成員收到 `EMP4` 計為 `malformed`，手冊寫明要先升級所有成員再設 `cipher`。

**Interface / data shape**

- `wire.rs`：`MAGIC_V4 = b"EMP4"`、`OVERHEAD_V4 = 4 + 8 + 8 + 16`、`MIN_DATAGRAM_V4 = OVERHEAD_V4 + 2`；`payload_for(msg, OVERHEAD_V4)` 截短 reason，`emp3` 欄位不寫。
- `crypto.rs`：`encode_encrypted_v4(&Message, &Secret, boot: u64, counter: u64) -> Vec<u8>`（或等價的函式，簽名在實作時定）、`Keys::open_v4`、`open_with` 認得 `EMP4`。
- `group.rs`：`Group::set_cipher(Cipher)`、`Cipher` enum 於 `config.rs`；`Format::EncryptedV4`。
- `config.rs`：`PeersConfig.cipher`。

**Failure modes**

- 解不開（金鑰錯、被改動）計 `auth`；長度、標頭 `boot` 與 payload 不符、payload 不是合法 JSON 計 `malformed`；沒有新的拒絕原因。
- 計數器是 `u64`，不會繞回；若真到上限（`u64::MAX`），送出者停止送 `EMP4` 並 `warn`，不重複 nonce。這是不可達的防護，實作時以單元測試覆蓋。

**Acceptance criteria**

- `crypto.rs` 與 `wire.rs` 單元測試：`EMP4` 往返、竄改任何一個 byte（`boot`、計數器、密文、標記）、長度邊界 37／38／1200／1201、前一個 secret 30／90 秒接受與 130 秒拒絕、同一送出者連續 datagram 的 (金鑰, nonce) 不重複、兩個不同 `boot` 的金鑰不同。
- 已知答案測試：`crates/emfor-daemon/tests/emp2_kat.py` 的做法擴充一份獨立實作（Python 的 `cryptography` 若 Debian 映像沒有，則以純 Python AES-GCM 對 NIST 測試向量 gcm-spec Test Case 16 驗證後，算出固定 secret、`boot`、計數器與 payload 的 `EMP4` bytes）；`crypto.rs` 的測試比對這份 bytes。
- `config.rs` 測試涵蓋 `cipher` 範例表每一列；`group.rs` 測試涵蓋 `cipher` 切換後的格式、`EMP4` 不改變 `EMP2`／`EMP3` 的選擇、`accept_plaintext = false` 接受 `EMP4`；`audit` 測試涵蓋 `field=cipher`。
- `crates/emfor-daemon/tests/peers.rs`：兩個 daemon 都設 `cipher = "aes-256-gcm"`，互相 live，且透過網路抓到的 datagram 以 `EMP4` 開頭；其中一個 reload 改回預設後仍 live。
- `tools/docker-test.sh` 通過；`python3 tools/check-manual-links.py` 回報 0 problems。

**Scope boundaries**

- In scope：`EMP4` 格式與金鑰、`cipher` 設定與 reload、接收、稽核欄位、手冊與 CHANGELOG、peer-group 與 service-config 的 spec。
- Out of scope：FIPS build、移除 `ring`、MD5、自動協商、停止接受舊格式、client。

## Risks / Trade-offs

- **boot 碰撞：** 64 bits 隨機的 `boot` 若在同一個聯防群組內碰巧相同，兩個送出者共用金鑰；兩者的計數器都從 1 開始，nonce 就會重複，AES-GCM 重複 nonce 會洩漏 XOR 並破壞驗證。機率約為群組成員對數除以 2^64（成員上限 64，約 2^12 對），可以忽略，且與現有 `boot` 作為 replay 識別的假設一致；接受這個風險，不加額外防護。
- **綁到 C 實作：** daemon 的聯防程式碼依賴 aws-lc-rs；TLS 已經依賴，沒有新增建置工具。
- **不是 FIPS：** 預設版 aws-lc-rs 不是驗證過的模組，所以這個變更不會讓 emfor 符合 FIPS；手冊明講。
- **升級順序：** 先升級全部成員再設 `cipher`；順序反了，舊版成員看到的全是 `malformed`，被當成不在線。手冊寫明，也寫明復原方法（把 `cipher` 改回預設，reload）。
- **明文標頭：** `boot` 與計數器是明文，旁觀者看得到成員重啟與送出份數，看不到內容。

## Migration Plan

1. 把所有成員升級到認得 `EMP4` 的版本（`cipher` 不設，行為不變）。
2. 逐台把 `cipher = "aes-256-gcm"` 寫進設定並 reload。順序不限：已改的成員送 `EMP4`，所有成員都收得到；沒改的繼續送 `EMP2`／`EMP3`，改了的也收得到。
3. 要退回時把 `cipher` 改回預設並 reload；不需要重啟。
