## Why

RADIUS 是電信與 ISP 機房裡認證（AAA）的核心：BRAS、Wi-Fi 控制器、VPN 閘道都靠它，RADIUS 伺服器停擺時用戶就上不了網。它走 UDP，請求要用共享密鑰簽章，回應要驗證，內建的 `tcp` 檢查與 `exec` 都做不好。RFC 5997 定義了專門給健康檢查用的 Status-Server，FreeRADIUS、Radiator 等主流伺服器都支援。

## What Changes

- `emfor-check` 新增外掛引數 `radius`：`command = ["/usr/local/libexec/emfor-check", "radius"]`。
- 預設以 RFC 5997 Status-Server 檢查：送往認證埠（`application = "auth"`，預期 Access-Accept）或計費埠（`application = "acct"`，預期 Accounting-Response）。
- 也可用 `request = "access-request"` 以測試帳號做真正的 PAP 認證（`user` 加 `password_file`），確認後端的使用者資料庫也正常；`reject_is_up = true` 時 Access-Reject 也算 up（只確認伺服器有回應）。
- 共享密鑰只能用 `secret_file`（絕對路徑，每次檢查重讀），不接受寫在設定檔裡。每個請求都帶 Message-Authenticator；回應的 Response Authenticator 一律驗證，回應帶 Message-Authenticator 時也驗證，`require_message_authenticator = true` 時沒帶即為 down（防範 BlastRADIUS 類攻擊）。
- 原因：`auth: Access-Reject`（伺服器有 Reply-Message 時附上）、`auth: response authenticator does not match (wrong secret?)`、`query: ` 為格式錯誤或非預期的回應碼；其餘沿用 `connect: `、`timeout`；`secret_file: `、`password_file: ` 讀不到為 error。
- 使用 emfor-check-dns 建立的 UDP 收發（逾時一半重送一次）。MD5 與 HMAC-MD5 用 `md-5` 0.11 與 `hmac` 0.13，兩者都已在 `Cargo.lock`，不新增套件。
- 實機測試：`emfor-plugins-test` 映像加入 FreeRADIUS（開啟 status_server）；重建映像前先向使用者確認。
- 文件：`docs/deploy/plugins.md` 加入 radius 一節（含 FreeRADIUS 開啟 Status-Server、把 emfor 主機加入 clients 的設定）；`examples/emfor.toml` 加入註解掉的範例。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 RadSec（RADIUS over TLS / TCP）、CHAP 與 EAP、CoA / Disconnect、比對回應屬性內容。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `network-checks`（由 emfor-check-dns 建立）：新增「RADIUS check options」與「RADIUS check」需求。
- `database-checks`：「Plugin selection by argument」加入 `radius`。

## Impact

- 程式：crates/emfor-check/src/lib.rs（`Kind` 加入 radius）、新增 crates/emfor-check/src/radius.rs、crates/emfor-check/Cargo.toml（`md-5`、`hmac`）。
- 測試：crates/emfor-check/tests/cli.rs、crates/emfor-check/tests/live.rs、tests/plugins/Dockerfile、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、examples/emfor.toml、docs/spectra/config.yaml、CHANGELOG.md。
- 前置：emfor-check-dns 必須先完成並 archive（本 change 使用它的 UDP 收發與 `network-checks` 規格，且「Plugin selection by argument」的修改以它為基礎）。
