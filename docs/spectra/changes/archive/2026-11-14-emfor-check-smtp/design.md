## Context

`emfor-check` 的網路 plugin（dns、radius、sip、diameter）各自解析選項、自己編解碼協定、不新增相依套件。TLS 共用 emfor-tls 與 `options::tls_settings`。ldap 已經有 `starttls`：先明文要求升級，被拒時原因是 `tls: StartTLS refused: ...`，而且不會改用明文繼續。

SMTP（RFC 5321）的回覆是三位數代碼加文字，多行時中間的行以 `NNN-` 開頭，最後一行以 `NNN ` 開頭。STARTTLS 定義在 RFC 3207，SMTP over TLS（465 port）定義在 RFC 8314。

## Goals / Non-Goals

**Goals:**

- 確認伺服器願意接受一個 SMTP 交談：招呼為 2xx、EHLO 被接受；設了 `starttls` 時 TLS 也能建立；
- 原因能指出是哪一步失敗。

**Non-Goals:**

- AUTH、`MAIL FROM`／`RCPT TO`、真的送信；
- 檢查 EHLO 宣告的其他能力（SIZE、PIPELINING 等）；
- 用戶端憑證、LMTP。

## Decisions

### 交談步驟固定為招呼、EHLO、可選 STARTTLS、QUIT

每一步只接受 2xx 或該步驟規定的代碼：

- 招呼必須是 220；
- `EHLO` 必須是 250；
- `STARTTLS` 必須是 220。

成功後送 `QUIT`，不等 221 就關閉連線，因為結果已經確定，等待只會拉長檢查時間。

替代方案：EHLO 被拒時改送 HELO。否決，因為現代伺服器都支援 EHLO，被拒通常代表伺服器設定有問題，應該是 `down`。

### starttls 要求 EHLO 宣告 STARTTLS

第一次 EHLO 的回覆沒有 `STARTTLS` 這一行（不分大小寫）時，為 `down`，原因 `tls: STARTTLS not offered`，不送 STARTTLS。伺服器回非 220 時為 `tls: STARTTLS refused: <代碼> <文字>`，與 ldap 的寫法一致。兩種情況都不會改用明文繼續。TLS 建立後再送一次 EHLO，因為 RFC 3207 規定升級後先前的交談狀態全部作廢。

### helo 預設用本機位址

`EHLO` 的參數預設是連線本機端位址的 address literal：IPv4 為 `[192.0.2.5]`，IPv6 為 `[IPv6:2001:db8::5]`。這是 RFC 5321 允許、又不必設定就正確的值。有些伺服器會檢查 EHLO 名稱，所以提供 `helo` 選項讓維運人員改成主機名稱。

### 回覆解析有長度上限

每一行最多 2048 位元組；一個回覆（含多行）最多 65536 位元組，超過為 `query: reply too long`。行尾接受 CRLF 或只有 LF。代碼不是三位數、續行代碼與第一行不同、或連線在回覆結束前關閉，都是 `query: malformed reply` 或 `query: connection closed`。原因只取回覆第一行的代碼與文字。

## Implementation Contract

**行為**

- 每次檢查開一條新連線：
  - `tls = true` 時先完成 TLS 握手；
  - 讀招呼 → `EHLO <helo>`；
  - `starttls = true` 時 → `STARTTLS` → TLS 握手 → `EHLO <helo>`；
  - 最後 `QUIT`。
- 全部成功為 `up`，否則為 `down`。

**選項**

- `helo`：1 到 255 位元組的主機名稱，只能有英數字、`-` 與 `.`，否則為 `must be a host name`；
- `tls`、`starttls`：布林值；兩者都為 true 時，`starttls` 的錯誤為 `conflicts with tls`；
- `tls_verify`、`server_name`、`ca_file`：沒有 `tls` 或 `starttls` 時為 `requires tls = true or starttls = true`；
- 其他 key 為 `unknown key`。

**原因**

- `greeting: <代碼> <文字>`
- `ehlo: <代碼> <文字>`
- `tls: STARTTLS not offered`
- `tls: STARTTLS refused: <代碼> <文字>`
- `tls: <握手或憑證錯誤>`
- `query: malformed reply`、`query: reply too long`、`query: connection closed`
- `connect: ...`、`timeout`
- 寫入失敗為 `query: ` 加上 I/O 錯誤

**驗收**

- smtp.rs 單元測試以本機假伺服器涵蓋 spec 範例表的每一列；拿掉 STARTTLS 宣告的檢查時，對應測試會失敗；
- tests/plugins/run.sh 對 Postfix 驗證：
  - 明文 port 以預設選項為 `up`；
  - 以 `starttls = true` 加測試 CA 為 `up`；
  - 465 wrapper mode 以 `tls = true` 為 `up`；
  - 沒有開 STARTTLS 的 port 以 `starttls = true` 為 `tls: STARTTLS not offered`；
- tests/cli.rs 的 handshake 測試、macOS 與 Debian 映像的 fmt、clippy、test 全部通過。

**範圍**

- 範圍內：smtp plugin、選項、文件、範例、CHANGELOG、實機測試；
- 範圍外：Non-Goals 所列各項。

## Risks / Trade-offs

- [伺服器對頻繁的連線做速率限制（例如 Postfix 的 `smtpd_client_connection_rate_limit`）] → 文件提醒把 emfor 主機加入例外，或拉長 `interval`。
- [每次檢查都會在郵件伺服器的 log 留下一筆沒有送信的連線] → 文件說明這是預期行為。
- [Dockerfile 加 postfix 需要重建 emfor-plugins-test 映像（下載套件）] → 實作時先徵求同意。
