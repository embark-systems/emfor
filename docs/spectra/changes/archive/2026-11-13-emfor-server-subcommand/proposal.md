## Why

目前不帶子命令的 `emfor` 就會啟動 daemon。使用者在 FreeBSD 上直接執行 `emfor`，本來只想看用法，結果啟動了第二個 daemon，得到 `store is already in use by another emfor daemon` 的錯誤。daemon 已經有 `emfor status` 子命令，啟動 daemon 和檢查設定也應該是明確的子命令；`emfor` 本身只印用法。

## What Changes

- **BREAKING**：新增子命令 `emfor server` 啟動 daemon，行為與現在不帶子命令的 `emfor` 相同（含結束碼、SIGHUP、SIGTERM）。
- **BREAKING**：不帶子命令的 `emfor` 印出用法到 stderr、以 2 結束，不讀設定檔、不開 store、不啟動外掛。
- **BREAKING**：`--check-config` 旗標移除，改為子命令 `emfor check-config`，輸出與結束碼不變。
- `--config` 維持全域選項，`emfor server`、`emfor check-config`、`emfor status` 都可用，寫在子命令前後皆可。
- 範例與部署文件改用新命令：`examples/rc.d/emfor`、`examples/systemd/emfor.service`、`examples/emfor.toml` 開頭的說明、`examples/freebsd/`、`docs/deploy/`、`docs/plugins/protocol-v1.md`、兩份 README。
- 測試與測試腳本改用新命令：`crates/emfor-daemon/tests/` 的整合測試、`tests/resolver/`、`tests/plugins/`、`tests/spike/run.sh`。
- CHANGELOG 最上方新增 `## Unreleased` 段，開頭寫明這是不相容的改動、舊用法要怎麼改；下一版依 RELEASING.md 發成 0.2.0（版本號在發版時才改，不在本 change 內）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`：新增「Command line subcommands」需求（`emfor` 印用法、`server`、`check-config`、全域 `--config`）；「Check-config command」與「Default config path set at build time」改用 `emfor check-config` 與 `emfor server`。
- `status-command`：「Status reads the store read-only」最後一句（不帶 `status` 的 `emfor` 維持原行為）改為指向新的子命令需求。
- `checker-plugins`：「Plugin validation of check options」中的 `--check-config` 改為 `emfor check-config`。
- `dns-server`：「Services with backends on different ports are reported」中的 `--check-config` 改為 `emfor check-config`。
- `database-checks`：「Check options validation」情境中的 `emfor --check-config` 改為 `emfor check-config`。

## Impact

- 程式：crates/emfor-daemon/src/cli.rs（clap 定義與 main 的分派）
- 測試：crates/emfor-daemon/src/cli.rs 的單元測試；crates/emfor-daemon/tests/daemon.rs、crates/emfor-daemon/tests/dns.rs、crates/emfor-daemon/tests/metrics.rs、crates/emfor-daemon/tests/plugins.rs
- 範例：examples/rc.d/emfor、examples/systemd/emfor.service、examples/emfor.toml、examples/freebsd/Makefile、examples/freebsd/README.md
- 文件：README.md、README.zh-TW.md、docs/deploy/daemon.md、docs/deploy/plugins.md、docs/plugins/protocol-v1.md、CHANGELOG.md
- 測試腳本：tests/resolver/inside.sh、tests/resolver/resolved.sh、tests/resolver/freebsd.sh、tests/plugins/ 中啟動 daemon 的腳本、tests/spike/run.sh
- 已部署的 0.1.x 主機：升級時要把服務設定中的 `emfor --config ...` 改成 `emfor server --config ...`，`--check-config` 改成 `check-config`
