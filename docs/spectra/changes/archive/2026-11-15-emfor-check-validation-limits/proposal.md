## Problem

commit 3ef4908 讓 `emfor-check` 在驗證 `check_options` 時多擋三種設定，但 specs 還沒寫進去：

- `pgsql` 的 `password_file` 密碼含 NUL：以前驗證會通過，之後每次檢查都是 `down`。
- `amqp` 的密碼長到 Start-Ok 放不進 4096 bytes 的第一個 frame：以前驗證會通過，每次檢查送出 broker 可能拒收的 frame。
- `diameter` 的 id 清單多到 Capabilities-Exchange-Request 超過 24-bit 的 Message Length：以前長度欄位會被靜默截斷。

database-checks 的 "Check options validation"、"Passwords from password_file only"，以及 network-checks 的 "Diameter check options"、"AMQP check options" 仍描述修正前的規則，和程式碼不一致。

## Root Cause

驗證只檢查值的型別與格式，沒有照檢查實際送出的內容去組一次訊息，所以協定本身的限制（PostgreSQL 字串不能含 NUL、AMQP 0-9-1 的 frame-min-size、Diameter 的 24-bit 長度）只在檢查時才出錯，而且看起來像後端故障。

## Proposed Solution

只更新 specs，讓它們描述已經上線的行為；程式碼與測試已在 3ef4908 與 47cf22c 完成並推上 origin。

- database-checks "Check options validation"：`pgsql` 的 `password_file` 在驗證時，密碼含 NUL 為 `password must not contain NUL`；範例表加一列。
- database-checks "Passwords from password_file only"：`pgsql` 檢查時讀到含 NUL 的密碼，在連線前回報 `error`，reason 為 `password_file: password must not contain NUL`。
- network-checks "AMQP check options"：驗證時組出 Start-Ok frame，超過 4096 bytes 為 `password too long: Start-Ok must fit in AMQP's 4096-byte first frame, at most <n> bytes with this user`；檢查時密碼檔變長則在連線前回報 `error`，reason 以 `password_file: password too long` 開頭。
- network-checks "Diameter check options"：驗證時組出 Capabilities-Exchange-Request（`host_ip_address` 未設時以 IPv6 位址計算），超過 16777215 bytes 時在最長的 id 清單上回報 `too many entries: the Capabilities-Exchange-Request would be <n> bytes, above Diameter's 16777215-byte message limit`。

## Non-Goals (optional)

- 不改程式碼或測試；不重跑實作。
- 不把 diameter 的上限調成 freeDiameter 實測的 65535 bytes：那是單一實作的限制，已寫在 docs/deploy/plugins.md。
- 不處理 `mysql` 密碼含 NUL（mysql_async 送密碼的方式另議）。
- 不在 `amqp` 檢查時重驗密碼的 NUL（驗證時已擋，與修正前相同）。

## Success Criteria

- 四條需求的文字與範例和 `crates/emfor-check/src/options.rs`、`pgsql.rs`、`amqp.rs`、`diameter.rs` 的行為一致，錯誤訊息逐字相同。
- 每個新增的 scenario 都對應到已存在的測試：`options::tests::option_validation_table`（pgsql NUL 密碼列）、`pgsql::tests::password_with_nul_is_error`、`amqp::tests::amqp_login_fits_the_first_frame`、`amqp::tests::too_long_password_is_error`、`diameter::tests::capabilities_exchange_request_fits_the_length_field`，以及 live 測試 `amqp_live`、`diameter_live`。
- `spectra validate emfor-check-validation-limits` 通過。

## Impact

- Affected specs: database-checks, network-checks
- Affected code:
  - Modified: docs/spectra/specs/database-checks/spec.md, docs/spectra/specs/network-checks/spec.md（歸檔時套用）
  - New: none
  - Removed: none
