## Context

`crates/emfor-daemon/src/peers.rs` 以 `encode(msg, key)` 產生 `EMP1` datagram（`EMP1`、HMAC-SHA256 標記、JSON payload），以 `decode` 解析並依 peer-group requirement「Peer message authentication and replay protection」的順序檢查：`malformed`、`auth`、`sender`、`site`、`stale`、`replay`。`Keys` 保存目前與前一個 secret：`signing(now)` 在換 secret 後 60 秒內仍用舊的簽，接收端 120 秒內接受舊的。`fuzz/fuzz_targets/peers.rs` 透過 `peers::fuzz_decode` 餵入任意 bytes。daemon 已依賴 RustCrypto 的 `hmac` 0.13 與 `sha2` 0.11。

payload 是明文 JSON，能看到流量的人可以讀到 site、成員、服務 tag、backend 位址與檢查結果。

## Goals / Non-Goals

**Goals:**

- payload 加密，竄改與重放照樣被擋下。
- 沿用 `secret_file`、secret 輪替、重放檢查與 1200 bytes 上限。
- peer group 可以逐台升級，不必同時停機。

**Non-Goals:**

- QUIC／DTLS、前向保密、隱藏長度與送出時間、移除 `EMP1`。

## Decisions

### EMP2：ChaCha20-Poly1305，金鑰由 HKDF 導出

datagram 為 `EMP2` ‖ nonce（12 bytes，每則訊息由 OS 亂數產生）‖ ChaCha20-Poly1305 密文 ‖ 16 bytes 標記。附加驗證資料（AAD）是 `EMP2` 四個 bytes。金鑰為 `HKDF-SHA256(salt = "emfor peers", ikm = secret, info = "EMP2 chacha20-poly1305")` 的前 32 bytes，每個 secret 導出一次並與它一起存在 `Keys`。

AEAD 同時提供機密性與完整性，所以 `EMP2` 不再帶 HMAC。標記驗不過時計為 `auth`，與 `EMP1` 的 HMAC 不符相同。

隨機 nonce 不需要跨重啟保存狀態。以每秒每成員一則、64 個成員估計（未量測，依 spec 上限推算），約每秒 64 則；96 bits 隨機 nonce 在同一把金鑰下要到約 2^48 則才有可觀的碰撞機率，遠超過 secret 合理的使用期。

替代方案：nonce 由 `boot` 與 `seq` 組成。可省 12 bytes，但 nonce 的唯一性取決於 `boot` 不重複且 `seq` 不回繞，出錯時代價是金鑰外洩等級的問題；隨機 nonce 比較單純。不採用。

### 用 RustCrypto 的 chacha20poly1305 與 hkdf

與現有 `hmac`、`sha2` 同一家族，純 Rust，不需要 C 工具鏈，fuzz 建置也與現在相同。版本選擇與 `sha2` 0.11 相容的版本（同一個 `digest` 主版本），在 task 1.1 確認後鎖定。

替代方案：直接使用 rustls 已帶進來的 `aws-lc-rs`。可少一個依賴，但 daemon 的 peer 程式會綁定 C 實作與 rustls 的 crypto provider 選擇，不採用。

### 兩種格式都接受，送出哪一種由 encrypt 決定

接收端依前 4 bytes 分辨 `EMP1` 與 `EMP2`，兩種都接受，之後的 `sender`、`site`、`stale`、`replay` 檢查完全相同。`[peers] encrypt`（預設 `true`）決定送出的格式。`encrypt = true` 時收到 `EMP1` 照樣接受，但每個來源每 60 秒記一行 `warn`：`peer <addr> sends plaintext messages (EMP1); set encrypt = true there`。

這樣升級可以分兩步：先在新版設 `encrypt = false`，逐台升級（新舊版都送也都懂 `EMP1`）；全部升級後把 `encrypt` 改為 `true` 並 reload，逐台切換時其他成員都接受 `EMP2`。

替代方案：`encrypt = true` 時拒收 `EMP1`。切換期間還沒改設定的成員會被當成不在線，觸發不必要的檢查重新分配。不採用；`warn` 已足以提醒。

### 預設 encrypt = true

新部署預設就加密。代價是直接升級新舊並存的 peer group 時，新版送出的 `EMP2` 會被舊版當成 `malformed` 丟掉，舊版會認為新版不在線，直到全部升級。手冊與 CHANGELOG 在升級說明中寫明「先設 `encrypt = false`」。

## Implementation Contract

**行為**

- `[peers] encrypt` 省略或為 `true`：送出 `EMP2`；`false`：送出 `EMP1`。reload 修改 `encrypt` 立即生效（下一則送出的訊息）。
- 接收：datagram 依前 4 bytes 為 `EMP1` 或 `EMP2` 分流；`EMP1` 的規則不變；`EMP2` 依下列順序檢查並計數：
  1. 長度 34 到 1200 bytes 且以 `EMP2` 開頭，否則 `malformed`；
  2. 以目前 secret 的金鑰，或 120 秒內被取代的前一個 secret 的金鑰解密並驗證成功，否則 `auth`；
  3. 解出的 payload 是含所有共同欄位、且已知 `kind` 的欄位都合法的 JSON 物件，否則 `malformed`；
  4. 其後 `sender`、`site`、`stale`、`replay` 與 `EMP1` 相同。
- 其他前綴計為 `malformed`。
- `encrypt = true` 時接受了 `EMP1`：每個來源每 60 秒最多一行 `warn`，內容如決策所述。
- 加密 payload 上限為 1200 − 4 − 12 − 16 = 1168 bytes；heartbeat 的 `off` 上限 80 個 tag 不變，且 80 個 tag 的 heartbeat 在兩種格式下都不超過 1200 bytes。

**設定錯誤**

- `encrypt` 不是 boolean：`peers.encrypt`：`encrypt must be true or false`。

**介面**

- `peers::encode(msg, keys/format)` 依格式產生 datagram；`Keys` 為每個 secret 保存導出的 EMP2 金鑰。
- `peers::fuzz_decode` 也涵蓋 `EMP2`：fuzz 以固定的測試 secret 解密。

**驗收**

- 單元測試：`EMP2` 往返、竄改任一 byte（含 nonce 與 AAD）計為 `auth`、前一個 secret 30／90 秒接受與 130 秒拒絕、長度邊界 33／34／1200／1201、80 個 tag 的 heartbeat 不超過 1200 bytes、`encrypt = true` 收到 `EMP1` 的 `warn` 限流。
- 已知答案測試：固定 secret、nonce 與 payload 的 `EMP2` bytes，由 `crates/emfor-daemon/tests/emp2_kat.py` 獨立計算。該腳本以純 Python 依 RFC 5869（HKDF-SHA256）與 RFC 8439（ChaCha20-Poly1305）實作，執行時先對照兩份 RFC 的測試向量；改用它是因為開發機與 Debian 容器都沒有安裝 Python `cryptography`。
- `crates/emfor-daemon/tests/peers.rs`：兩個 daemon 都 `encrypt = true` 時互相在線；一個 `true`、一個 `false` 時互相在線且 `true` 那邊記 `warn`；reload 把 `encrypt` 從 `false` 改為 `true` 後送出 `EMP2`（用抓包 socket 驗證前綴）。
- 每個新測試在拿掉對應行為時失敗。
- `peers` fuzz target 跑 60 秒沒有新 artifact。

**範圍**

- 在範圍內：`crates/emfor-daemon` 的 peers 與 config、fuzz、手冊第 3 與 8 章、`docs/deploy/daemon.md`、`examples/emfor.toml`、CHANGELOG。
- 不在範圍內：store、client、metrics 名稱、`emfor status` 的輸出。

## Risks / Trade-offs

- [直接升級新舊並存的 peer group 時，新版暫時被舊版當成不在線] → 升級說明要求先設 `encrypt = false`；CHANGELOG 標明。
- [secret 外洩時舊流量可被解密] → 沒有前向保密；手冊建議定期換 secret（現有輪替機制不需停機）。
- [`encrypt = true` 仍接受 `EMP1`，攻擊者若有 secret 可送明文] → 有 secret 本來就能送合法 `EMP2`，不增加風險；`warn` 會顯示來源。
- [加密增加 CPU] → 每秒每成員一則、約 1 KB，ChaCha20-Poly1305 的成本可忽略（估計，未量測；可由 `peers` 單元測試的微基準確認）。
