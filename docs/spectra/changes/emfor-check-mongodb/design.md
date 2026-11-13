## Context

MongoDB 3.6 起用 OP_MSG（opCode 2013）傳送指令，訊息的格式是：

- 16 位元組的標頭：messageLength、requestID、responseTo、opCode；
- 4 位元組的 flagBits；
- 一個 kind 0 的 section，內容是一份 BSON 文件。

`hello` 指令（4.4.2 起，4.2.10 也有）不需登入，回覆中有：

- `isWritablePrimary`、`secondary`、`arbiterOnly`；
- `setName`（replica set 名稱）；
- `msg`（mongos 為 `isdbgrid`）。

指令失敗時回 `ok: 0`，附 `code`、`codeName`、`errmsg`。

登入用 SCRAM-SHA-256（RFC 7677）：

1. `saslStart` 送 client-first-message；
2. 伺服器回 nonce、salt、iteration count；
3. `saslContinue` 送 client-final-message（含 proof）；
4. 伺服器回 server signature（`v=`），用戶端必須驗證。

`emfor-check` 的其他 plugin 不依賴大型驅動程式時，協定都自己寫；redis 與 ldap 就是如此。

## Goals / Non-Goals

**Goals:**

- 確認伺服器回應 `hello`、帳號能登入、角色符合；
- `role = "primary"` 搭配 `balance = "failover"` 時跟著 replica set 的 primary 走；
- 不引入 MongoDB 官方驅動（相依多、會自己做連線池與伺服器探索，與每次檢查開新連線的模式不合）。

**Non-Goals:**

- SCRAM-SHA-1、x.509、LDAP、Kerberos 等其他登入方式；
- 非 ASCII 密碼（需要 SASLprep）；
- 複寫延遲（`max_lag`，需 `replSetGetStatus` 的權限）；
- 4.2.10 以前、沒有 `hello` 的版本；
- 連線字串（`mongodb://`）與 SRV 探索。

## Decisions

### 先送 hello，有帳號再登入

`hello` 不需登入，能確認伺服器活著並取得角色，所以每次檢查都先送 `{hello: 1, $db: "admin"}`。設了 `user` 時再做 SCRAM-SHA-256 登入，`$db` 是 `auth_source`。角色永遠以 `hello` 的回覆判斷。沒設帳號時只做 `hello`，適合沒開認證、或只想確認角色的部署。

### BSON 只做需要的型別

編碼只產生：文件、字串、int32、布林、binary（subtype 0，用於 SASL payload）。

解碼認得的型別有：double、字串、文件、陣列、binary、布林、null、int32、timestamp、int64、ObjectId、datetime、decimal128，其他型別都可以跳過。遇到不認得的型別碼時為 `query: malformed reply`。回覆總長上限 65536 位元組，超過為 `query: reply too large`。flagBits 的 checksumPresent 位元若被設定，忽略最後 4 位元組。

### SCRAM-SHA-256 自己實作並用 RFC 7677 範例測試

- PBKDF2 以既有的 `hmac` 加 `sha2` 自己寫幾行，不另加 `pbkdf2` 套件；
- client nonce 是 24 個隨機位元組，經 base64 編碼；
- 使用者名稱中的 `=` 與 `,` 依 RFC 5802 轉義；
- 伺服器的 iteration count 小於 4096 時為 `auth: malformed SCRAM message`；
- server signature 不符時為 `auth: server signature does not match`；
- `saslStart` 帶 `options: { skipEmptyExchange: true }`，讓伺服器在第二步就回 `done: true`。

正確性以 RFC 7677 第 3 節的範例（user `user`、password `pencil`、固定 nonce）做單元測試。

替代方案：用 `scram` 或 `pbkdf2` 套件。否決，因為需要下載新套件，而實作只有幾十行，且有標準範例可以測。

### 密碼限定可列印 ASCII

MongoDB 對 SCRAM-SHA-256 的密碼做 SASLprep。可列印 ASCII 經過 SASLprep 不會改變，所以只支援這個範圍，不必實作 SASLprep。密碼含其他字元時，檢查結果為 `error`（plugin 這邊的限制，不算後端失敗），原因 `password_file: only printable ASCII passwords are supported`。

### 角色取自 hello 的欄位

實際角色依下列順序判斷：

1. `arbiterOnly` 為 true：`arbiter`；
2. `msg` 為 `isdbgrid`：`mongos`；
3. `isWritablePrimary` 為 true：有 `setName` 時為 `primary`，沒有時為 `standalone`；
4. `secondary` 為 true：`secondary`；
5. 其他：`unknown`（例如 RECOVERING、STARTUP2）。

比對規則：

- `role = "primary"` 接受 `primary`、`standalone`、`mongos`，因為這三種都能寫入；
- `role = "secondary"` 只接受 `secondary`；
- 不符時原因為 `role is <實際>, want <要求>`。

用 `secondary` 而不用其他 plugin 的 `replica`，是沿用 MongoDB 自己的用語，和 redis 用 `master` 的做法一致。

## Implementation Contract

**行為**

- 每次檢查開一條新連線（`tls = true` 時先 TLS 握手）；
- 送 `hello` → 有 `user` 時以 SCRAM-SHA-256 登入 → 依 `role` 比對；
- 全部成功為 `up`。

**選項**

- `user`：1 到 255 位元組；
- `password_file`：規則與其他 plugin 相同；
- `user` 與 `password_file` 必須同時設：只設 `user` 為 `requires password_file`，只設 `password_file` 為 `requires user`；
- `auth_source`：1 到 64 位元組，只能有英數字、`_` 與 `-`，預設 `admin`，否則為 `must be a database name`；沒有 `user` 時為 `requires user`；
- `role`：`primary` 或 `secondary`，否則為 `must be one of primary, secondary`；
- TLS 鍵：規則與 redis、mysql、pgsql 相同；
- 其他 key 為 `unknown key`。

**原因**

- `auth: <codeName>: <errmsg>`
- `auth: server signature does not match`、`auth: malformed SCRAM message`
- `role is <實際>, want <要求>`
- `query: <codeName>: <errmsg>`
- `query: malformed reply`、`query: reply too large`、`query: connection closed`
- `connect: ...`、`tls: ...`、`timeout`
- `password_file: only printable ASCII passwords are supported`（`error`）

**驗收**

- mongodb.rs 單元測試：
  - BSON 編解碼來回；
  - RFC 7677 範例的 client-final-message 與 server signature；
  - 以本機假伺服器涵蓋 spec 範例表每一列；
  - 拿掉 server signature 驗證時，對應測試會失敗；
- tests/plugins/run.sh 對 MongoDB 驗證：
  - replica set 的 primary 以 `role = "primary"` 為 `up`；
  - secondary 以 `role = "primary"` 為 `role is secondary, want primary`；
  - secondary 以 `role = "secondary"` 為 `up`；
  - 需登入的伺服器以正確帳密為 `up`，以錯誤密碼為 `auth: AuthenticationFailed: ...`；
  - TLS port 以 `tls = true` 加測試 CA 為 `up`；
- tests/cli.rs 的 handshake 測試、macOS 與 Debian 映像的 fmt、clippy、test 全部通過。

**範圍**

- 範圍內：mongodb plugin、選項、文件、範例、CHANGELOG、實機測試；
- 範圍外：Non-Goals 所列各項。

## Risks / Trade-offs

- [自己實作 SCRAM 有出錯的風險] → 用 RFC 7677 範例做單元測試，並在實機測試中對真的 MongoDB 登入。
- [MongoDB 不在 Debian 套件庫中，需要從 MongoDB 官方套件庫下載，映像會變大] → 實作時先徵求同意再改 Dockerfile 與重建映像；記錄驗證用的 MongoDB 版本。
- [`hello` 在 4.2.10 以前的版本不存在] → 原因會是 `query: CommandNotFound: ...`，文件寫明最低版本。
