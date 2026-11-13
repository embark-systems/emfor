## Why

Diameter 是 4G / 5G 核心網的 AAA 與計費協定：HSS（S6a）、PCRF（Gx）、OCS（Gy）、DRA 之間都靠它。Diameter 節點能接受 TCP 連線，不代表它會接受新的 peer：它可能不認得對方的 Origin-Host、沒有共同的 application，或正在過載。真正的健康檢查要走完能力交換（CER / CEA）再送 Device-Watchdog（DWR / DWA）。這是二進位協定，內建的 `tcp` 檢查做不到。

## What Changes

- `emfor-check` 新增外掛引數 `diameter`：`command = ["/usr/local/libexec/emfor-check", "diameter"]`。
- 每次檢查開一條 TCP 連線（可選 TLS），依序送 CER，收到成功的 CEA 後送 DWR；DWA 成功即為 up，之後送 DPR（Disconnect-Cause `DO_NOT_WANT_TO_TALK_TO_YOU`）並關閉連線。
- 必填選項：`origin_host`、`origin_realm`，以及至少一個 application（`auth_application_ids`、`acct_application_ids` 或 `vendor_specific_application_ids`，後者給 3GPP 等需要 Vendor-Id 的 application）；可選 `supported_vendor_ids`、`host_ip_address`（預設為本機連線位址）、TLS 選項（與其他外掛同名同義），以及 diameter 專用的客戶端憑證 `tls_cert_file`、`tls_key_file`（RFC 6733 的 TLS 要求雙向認證）。
- Result-Code 不是 2xxx 時為 down，原因帶代碼與 RFC 6733 的名稱，例如 `cea: 3010 DIAMETER_UNKNOWN_PEER`、`cea: 5010 DIAMETER_NO_COMMON_APPLICATION`、`dwa: 3004 DIAMETER_TOO_BUSY`；peer 在 CEA 前關閉連線為 `cea: connection closed by peer`；格式錯誤為 `query: `；其餘沿用 `connect: `、`timeout`、`tls: `。
- Diameter 訊息自己編解碼（標頭與 AVP 格式固定，只處理 CER/CEA、DWR/DWA、DPR 需要的 AVP），不新增套件。
- 實機測試：`emfor-plugins-test` 映像加入 freeDiameter（Debian 套件 freediameterd、freediameter-extensions）；重建映像前先向使用者確認。
- 文件：`docs/deploy/plugins.md` 加入 diameter 一節（含在對端把 emfor 加入允許的 peer 清單）；`examples/emfor.toml` 加入註解掉的範例。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 SCTP、送 application 層的請求（例如 S6a 的 AIR、Gx 的 CCR）、長連線的 peer 狀態機、DTLS。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `network-checks`（由 emfor-check-dns 建立）：新增「Diameter check options」與「Diameter check」需求。
- `database-checks`：「Plugin selection by argument」加入 `diameter`。

## Impact

- 程式：crates/emfor-check/src/lib.rs（`Kind` 加入 diameter）、crates/emfor-check/src/main.rs（usage）、crates/emfor-check/src/options.rs（`Kind` 的分支）、新增 crates/emfor-check/src/diameter.rs；TCP 與 TLS 沿用 crates/emfor-check/src/net.rs；crates/emfor-tls/src/lib.rs 新增客戶端憑證支援。
- 測試：crates/emfor-check/tests/cli.rs、crates/emfor-check/tests/live.rs、tests/plugins/Dockerfile、tests/plugins/inside.sh。
- 文件：docs/deploy/plugins.md、examples/emfor.toml、docs/manual/04-health-checks.md、docs/manual.zh_TW/04-health-checks.md、docs/spectra/config.yaml、CHANGELOG.md、crates/emfor-check/Cargo.toml（description）。
- 前置：emfor-check-dns、emfor-check-radius、emfor-check-sip 依序完成並 archive（使用 `network-checks` 規格與各外掛自行驗證選項的方式；「Plugin selection by argument」的修改以 SIP change 為基礎）。不使用 UDP 收發。
