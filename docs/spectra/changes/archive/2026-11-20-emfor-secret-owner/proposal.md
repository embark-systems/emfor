## Why

來自 CASA 自我評估（TODO 67 第 5 項）。`read_secret` 開啟 peer secret 檔後只檢查權限，沒檢查擁有者；載入 `include_dir` 片段的 `read_trusted` 則要求擁有者是 root 或 daemon 執行的使用者。由別的非特權使用者擁有的 peer secret，無論權限位元怎麼設，擁有者都能改寫（擁有者可以 chmod），而這個 secret 負責驗證並加密每一則 peer 訊息。

## What Changes

- `read_secret` 一併檢查它開啟的那個檔案（跟著連結走到的檔案）的擁有者：必須是 root 或 daemon 的有效使用者（effective uid）。否則 `secret_file`、`previous_secret_file` 是設定錯誤，訊息說明檔案屬於 uid N，不是 root 也不是執行 emfor 的使用者。規則與 `untrusted()` 對片段使用的相同。
- **BREAKING**：由別的帳號建立的 secret 檔（例如 `alice`、權限 0640、群組 `emfor`）原本會被接受，現在在啟動、`check-config` 與 reload 都會被拒絕。擁有者是 root 或 daemon 使用者的檔案，不論群組，權限 0600 或 0640 仍被接受。
- 手冊（英文與繁體中文，`[peers]` 表）與 CHANGELOG 說明這條規則；CHANGELOG 條目標明這是 0.7.0 的相容性變更。`docs/deploy/daemon.md` 沒有描述 secret 檔權限，不用改。

## Non-Goals

- 不改權限檢查（`o+r`、`go+w`）、一般檔案與 FIFO 檢查、secret 長度。
- 服務檢查的 secret 檔（`check_options` 裡的 `password_file`、`secret_file`、`tls_key_file`）的擁有者仍不檢查（credential warnings 只看權限，TODO 67 第 4 項）；本變更只管 peer secret。
- 不檢查放檔案的目錄。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`：在「Peer secret file must not be writable」旁新增要求「Peer secret file owner」，附情境與範例表。

## Impact

- `crates/emfor-daemon/src/config.rs`：`read_secret`，以及一個吃檔案 uid 與有效 uid 的純函式和它的測試。
- `docs/spectra/specs/peer-group/spec.md`（經由 delta spec）。
- `docs/manual/03-configuration.md`、`docs/manual.zh_TW/03-configuration.md`、`CHANGELOG.md`。
