## Context

`emfor-check`（emfor-check-plugins）已有完整的 TLS 實作，集中在 `crates/emfor-check/src/net.rs` 與 `options.rs`：`client_config`（信任 `ca_file` 或系統 CA、`tls_verify = false` 時用 `AcceptAnyCertificate`）、`trusted_certificates`（給 mysql_async 用）、`server_name`、`open`（TLS 握手最多用剩餘時間的 90%，不足 100 ms 時交給檢查本身的逾時）、`load_certificates` 與 `server_name` / `ca_file` 的驗證。這些函式直接回傳 emfor-plugin 的 `CheckOutcome`。

daemon 的 `tcp` 檢查器（`TcpChecker`）在 `emfor-tcp-expect` 之後支援 `send` / `expect`，但沒有 TLS。daemon 結果型別是 `CheckResult`，選項來源是 `toml::Table`；emfor-check 則是 `serde_json` 的 options。daemon 已透過 reqwest 連結 rustls（aws-lc-rs）與 tokio-rustls；`http` 檢查的 https 使用 reqwest 自帶的 rustls-platform-verifier，本 change 不改動它。

**前提**：本 change 以 `emfor-tcp-expect` 完成後的 `TcpChecker` 與「TCP checker」需求為基礎，須在 `emfor-tcp-expect` archive 之後才開始實作。

## Goals / Non-Goals

**Goals:**

- `tcp` 檢查可用 TLS 連線，驗證憑證與名稱，並可在加密連線上做 `send` / `expect`。
- TLS 選項的名稱、預設值、驗證訊息與 emfor-check 完全相同。
- 驗證憑證的程式碼只有一份，daemon 與 emfor-check 共用。

**Non-Goals:**

- client 憑證（mTLS）。
- 憑證到期前警告：過期的憑證本來就會讓握手失敗。
- STARTTLS 類的協定升級（SMTP、IMAP、LDAP 的明文升級）：tcp 檢查只支援一連上就 TLS。
- 改動 `http` 檢查的 TLS 實作。
- 改變 emfor-check 任何對外行為（選項、原因、驗證訊息）。

## Decisions

### 選項與 emfor-check 相同

`tls`、`tls_verify`、`server_name`、`ca_file` 的名稱、預設值、`requires tls = true` 規則與錯誤訊息都照 emfor-check 的 "Check options validation" 與 "TLS connections"。替代方案是沿用 `http` 檢查的 `tls_verify` 並另取名稱；但 tcp 的使用情境（任意 TLS 服務、以 IP 連線）與 emfor-check 相同，需要 `server_name` 與 `ca_file`，一致的寫法讓使用者只學一次。

### TLS 程式碼抽成 emfor-tls crate

新增 workspace 內部 crate `crates/emfor-tls`（不發布），內容為目前 emfor-check `net.rs` 與 `options.rs` 中和 TLS 有關的部分：`TlsSettings`、`client_config`、`trusted_certificates`、`server_name`、帶時間預算的握手、`AcceptAnyCertificate`、系統 CA 的載入與快取、`load_certificates`、`server_name` 與 `ca_file` 的檢查。替代方案是在 daemon 複製一份；驗證憑證的程式碼最怕兩份各自修改後行為不一致，因此共用。`emfor-plugin` 不適合放這些內容：它只處理外掛協定，daemon 也不相依它。

### emfor-tls 回傳自己的失敗型別

emfor-tls 不相依 emfor-plugin 或 daemon，函式回傳 `TlsFailure { local: bool, reason: String }`：`reason` 已含前綴（`tls: `、`ca_file: `），`local = true` 表示本機問題（讀不到 `ca_file`、沒有系統 CA、本機資源錯誤）。emfor-check 對應成 `CheckOutcome::Error` / `Down`，daemon 對應成 `CheckResult::Error` / `Down`。選項驗證則提供以值為輸入的檢查函式（例如 `check_server_name(&str)`、`check_ca_file(&Path)` 回傳錯誤訊息），由兩邊各自從 `toml` / `serde_json` 取值後呼叫，訊息因此一致。

### 握手後才做 send 與 expect

`tls = true` 時順序為 TCP 連線 → TLS 握手 → `send` → 讀取比對；沒有 `expect` 時握手成功即 up，送出 close_notify 後關閉。握手時間預算沿用 emfor-check 的規則（剩餘時間的 90%，不足 100 ms 時交給檢查本身的逾時），讓明文 port 回 `tls: ` 而非 `timeout`。

### 系統 CA 用 rustls-native-certs

未設 `ca_file` 時，tcp 檢查與 emfor-check 一樣用 rustls-native-certs 載入系統 CA（每個程序載入一次）。替代方案是像 `http` 檢查一樣用 rustls-platform-verifier；但它不支援 `ca_file` 取代系統 CA 的語意，且兩者混用會讓 tcp 與 emfor-check 在同一台主機上的信任結果不同。

## Implementation Contract

**行為**

- `tls` 未設或為 `false`：與 `emfor-tcp-expect` 完成後的 tcp 檢查完全相同。
- `tls = true`：連線 → 握手（時間預算如上）→ 有 `expect` 時 `send` / `expect`，否則 up 並送 close_notify 關閉。
- 握手或驗證失敗：down，原因 `tls: <rustls 錯誤>`；預算內無回應：down `tls: no answer to the TLS handshake (does the port speak TLS?)`；檢查時讀不到 `ca_file`：error `ca_file: <錯誤種類>`；沒有系統 CA 且未設 `ca_file`：error `tls: no system CA certificates found; set ca_file`。
- emfor-check：所有選項、原因、驗證訊息與現在相同。

**介面 / 資料形狀**

- `crates/emfor-tls` 公開：`TlsSettings { verify, server_name, ca_file }`、`TlsFailure { local, reason }`、`client_config(&TlsSettings) -> Result<Arc<ClientConfig>, TlsFailure>`、`trusted_certificates(&TlsSettings) -> Result<Vec<CertificateDer>, TlsFailure>`、`server_name(&TlsSettings, IpAddr) -> ServerName`、`handshake(TcpStream, &TlsSettings, IpAddr, remaining: Duration) -> Result<TlsStream<TcpStream>, TlsFailure>`、`load_certificates(&[u8])`、`check_server_name(&str) -> Result<(), &'static str>`、`check_ca_file(&Path) -> Result<(), String>`（訊息與現行 emfor-check 相同）。名稱可在實作時微調，但兩邊都只透過這個 crate 取得 TLS 行為。
- `TcpFactory::validate` 接受 `tls`、`tls_verify`、`server_name`、`ca_file`，錯誤鍵與訊息見 spec 的 "TLS validation errors" 表。

**驗收**

- emfor-tls 的單元測試：搬自 emfor-check `net.rs` 的 TLS 測試（rcgen 產生的 CA 與伺服器、名稱不符、`tls_verify = false`、沉默的明文伺服器、`little_time_left_is_not_blamed_on_tls`）全部通過。
- emfor-check：既有單元測試、`tests/cli.rs` 與 `tests/plugins/run.sh` 全部通過，證明行為不變。
- daemon `crates/emfor-daemon/src/checker/tcp.rs` 單元測試涵蓋 spec 新增的六個 TLS 情境與 "TLS validation errors" 表的七列，原有 tcp 測試全部通過。
- 本機檢查：macOS 與 Debian 映像的 fmt、clippy、test；Docker 中 `tests/resolver/run.sh dnsmasq`、`unbound` 與 `tests/plugins/run.sh`。

**範圍**

- 範圍內：emfor-tls crate、emfor-check 改用它、daemon tcp 檢查的 TLS、範例設定與 config.yaml。
- 範圍外：Non-Goals 所列項目；LMDB、client、DNS、外掛協定。

## Risks / Trade-offs

- [搬移時改變 emfor-check 的行為] → 先做搬移並跑完 emfor-check 全部測試與 `tests/plugins/run.sh`，再動 daemon。
- [daemon 多了 rustls 系列的直接相依，版本須與 reqwest 帶入的一致] → 全部用 workspace 已鎖定的版本；`cargo tree -d` 確認沒有重複的 rustls。
- [每次 TLS 檢查都做完整握手，比純 TCP 連線耗 CPU] → 與 emfor-check 相同取捨；檢查間隔通常為秒級，影響可忽略（未量測）。
- [`fastest` 服務的 `latency_us` 會包含握手時間] → 反映實際連線成本，屬預期。
