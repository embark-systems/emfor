## Why

`http` 檢查在 `scheme = "https"` 時只信系統 CA 庫。後端用私有 CA 簽發憑證（電信機房、封閉環境常見）時，只有兩條路：把私有 CA 裝進整台主機的系統信任庫，或設 `tls_verify = false`，後者等於完全不驗證憑證。其他所有帶 TLS 的檢查（`tcp`、`redis`、`mysql`、`pgsql`、`ldap`、`smtp`、各 plugin）都已有 `ca_file`，只有 `http` 沒有，是設定上的缺口。

## What Changes

- `http` 檢查新增選項 `ca_file`：PEM 檔路徑。設了就只信該檔裡的 CA，取代系統 CA 庫（像 `curl --cacert`）；沒設則維持現狀，用系統 CA 庫。
- 名稱與語意和其他檢查的 `ca_file` 相同；驗證用同一個函式（`emfor_tls::check_ca_file`），所以檔案讀不到、不是有效的 PEM 或沒有任何憑證時，設定檔驗證失敗（`check-config` 失敗），訊息與其他檢查一致。CA 檔不是機密，不套用密鑰檔的 mode 規則。
- `ca_file` 只對 `scheme = "https"` 有意義：`scheme = "http"` 時設了 `ca_file` 驗證失敗，訊息 `requires scheme = "https"`，作法同 `tcp` 的 `requires tls = true`。
- `ca_file` 與 `tls_verify = false` 同時設不算錯誤（`tcp` 檢查同樣允許）：`tls_verify = false` 優先，不驗證，`ca_file` 仍會被驗證格式。
- 手冊第 6 章 `http` 選項表與私有 CA 說明（中英）、第 16 章對 `tls_verify = false` 的建議、CHANGELOG 更新。

## Non-Goals

- 其他檢查的 `ca_file`：已經有，不動。
- client certificate（mTLS）：只有 `diameter` 有，不在此變更。
- `/metrics` 的存取控制：另一個 change。
- `http` 的 SNI 行為、`server_name` 選項：不動。
- 加在系統 CA 之上的模式（另一個選項）：已決定 `ca_file` 取代系統 CA，和其他檢查一致。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `health-checking`: requirement「HTTP checker」新增 `ca_file` 選項、它的驗證規則與信任語意。

## Impact

- Affected specs: health-checking
- Affected code:
  - Modified: crates/emfor-daemon/src/checker/http.rs, docs/manual/06-health-checks.md, docs/manual.zh_TW/06-health-checks.md, docs/manual/16-troubleshooting.md, docs/manual.zh_TW/16-troubleshooting.md, CHANGELOG.md
  - New: none
  - Removed: none
