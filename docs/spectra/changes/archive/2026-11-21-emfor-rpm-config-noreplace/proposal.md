## Why

rpm 套件把 `/etc/emfor/emfor.toml` 標成 `%config`（`crates/emfor-daemon/Cargo.toml` 的 `[package.metadata.generate-rpm]` 中 `config = true`），沒有 `noreplace`。rpm 對 `%config` 的語意是：套件新版的這個檔案與舊版不同，而管理者又改過磁碟上的檔案時，新版檔案會蓋掉磁碟上的檔案，舊的另存成 `emfor.toml.rpmsave`。套件附的 `emfor.toml` 只有 `include_dir`，所以管理者寫在主檔裡的 `[[services]]`、`[peers]`、`[store]` 會在升級時整個消失，而 `postun.sh` 接著 try-restart，daemon 用只有預設內容的設定重啟。同一個表裡的 `conf.d/README` 已經是 `config = "noreplace"`。這是 Fable 審查（2026-11-21）報告的項目；rpm 的語意是推論，還沒有實際升級驗證。

## What Changes

- 把 `emfor.toml` 那一行改成 `config = "noreplace"`，與同表的 `conf.d/README` 一致：升級時磁碟上改過的檔案保留，套件新版存成 `emfor.toml.rpmnew`。
- 建一個 rpm，用 `rpm -qp` 查看檔案旗標，確認 `emfor.toml` 帶 `noreplace`；盡可能在容器內用 0.6.0 的 rpm 當舊版、新建的 rpm 當新版，做一次真實升級（改過的 `emfor.toml` 保留，且出現 `.rpmnew`）。
- CHANGELOG 的 Unreleased 記一筆，並在手冊的升級段（`docs/manual/08-operations.md` 與 zh_TW）說明 `.rpmnew`。

## Non-Goals (optional)

- deb 不動：dpkg 對 `/etc` 的檔案本來就當 conffile，升級時會詢問而不是靜默覆蓋。
- 不改 `packages/common/emfor.toml` 的內容。

## Impact

- Affected specs: none
- Affected code:
  - New: (none)
  - Modified: `crates/emfor-daemon/Cargo.toml`（一行）、`CHANGELOG.md`、`docs/manual/08-operations.md`、`docs/manual.zh_TW/08-operations.md`
  - Removed: (none)
- Compatibility: 打包行為改變：從 0.6.0 的 rpm 升級到下一版時，管理者改過的主設定檔保留（0.6.0 本身已安裝的 rpm 帶 `%config` 旗標，升級到新版時仍依舊旗標處理，需要在驗證時確認這一步）。
