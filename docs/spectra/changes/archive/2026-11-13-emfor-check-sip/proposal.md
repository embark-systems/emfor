## Why

電信與 VoIP 機房裡的 SIP 伺服器（Kamailio、OpenSIPS、Asterisk、FreeSWITCH、SBC）是通話建立的入口，業界慣例是用 SIP OPTIONS 當心跳（Kamailio 的 dispatcher、各家 SBC 都這樣做）。SIP 主要走 UDP 5060，回應以狀態碼表示，內建的 `tcp` 檢查無法處理 UDP，`exec` 每次起程序又太重。

## What Changes

- `emfor-check` 新增外掛引數 `sip`：`command = ["/usr/local/libexec/emfor-check", "sip"]`。
- 每次檢查送一個 SIP OPTIONS 請求，最終回應的狀態碼在 `expect_status`（預設 `["200-499"]`）之內為 up；1xx 暫時回應忽略、繼續等最終回應。
- 傳輸：`transport = "udp"`（預設）或 `"tcp"`；`tls = true`（需要 `transport = "tcp"`）走 SIP over TLS，TLS 選項（`tls_verify`、`server_name`、`ca_file`）與其他外掛同名同義。
- 可設定 Request-URI（`uri`，預設 `sip:<後端 IP>:<port>`）與 From（`from`，預設 `sip:emfor@emfor.invalid`），配合只接受特定網域的伺服器。
- 原因：`status: 503 Service Unavailable` 之類（狀態碼與原因片語）、`query: ` 為格式錯誤或不相符的回應；其餘沿用 `connect: `、`timeout`、`tls: `。
- UDP 使用 emfor-check-dns 建立的 UDP 收發；回應以 Via branch、Call-ID 與 CSeq 比對。不新增套件（SIP 訊息自己組與解析，只處理 OPTIONS 需要的部分）。
- 實機測試：`emfor-plugins-test` 映像加入 Kamailio；重建映像前先向使用者確認。
- 文件：`docs/deploy/plugins.md` 加入 sip 一節；`examples/emfor.toml` 加入註解掉的範例。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 REGISTER、INVITE 等其他方法、Digest 認證、SCTP 與 WebSocket 傳輸、比對回應標頭或 SDP。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `network-checks`（由 emfor-check-dns 建立）：新增「SIP check options」與「SIP check」需求。
- `database-checks`：「Plugin selection by argument」加入 `sip`。

## Impact

- 程式：crates/emfor-check/src/lib.rs（`Kind` 加入 sip）、crates/emfor-check/src/main.rs（usage）、新增 crates/emfor-check/src/sip.rs、crates/emfor-check/src/net.rs（`udp_exchange_from`）、crates/emfor-check/src/options.rs（共用 `tls_settings` 與 `string`，dns.rs 與 radius.rs 改用）；TLS 沿用 crates/emfor-check/src/net.rs 與 emfor-tls。
- 測試：crates/emfor-check/tests/cli.rs、crates/emfor-check/tests/live.rs、tests/plugins/Dockerfile、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、examples/emfor.toml、docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、docs/spectra/config.yaml、CHANGELOG.md、crates/emfor-check/Cargo.toml（description）。
- 前置：emfor-check-dns 與 emfor-check-radius 依序完成並 archive（使用 DNS change 的 UDP 收發與 `network-checks` 規格；「Plugin selection by argument」的修改以 RADIUS change 為基礎）。
