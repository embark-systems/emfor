## 1. 修正

- [x] 1.1 `crates/emfor-daemon/Cargo.toml` 的 `[package.metadata.generate-rpm]` 中，`/etc/emfor/emfor.toml` 那一行由 `config = true` 改為 `config = "noreplace"`。完成時：`git diff` 只有這一行。
- [x] 1.2 [after: 1.1] 用 `packages/build.sh` 在 Docker 建 rpm，在 Rocky Linux 9 容器內以 `rpm -qp --queryformat '[%{FILEFLAGS:fflags} %{FILENAMES}\n]'` 查旗標：`/etc/emfor/emfor.toml` 帶 noreplace（`cn`），`conf.d/README` 同樣。完成時：把查到的輸出貼進 CHANGELOG 的 Checked。
- [x] 1.3 [after: 1.2] 在同一個容器內真實升級：先裝 `packages/dist` 內 0.6.0 的 rpm（舊旗標），改 `/etc/emfor/emfor.toml`，再升級到新建的 rpm，記錄結果（檔案是否保留、有沒有 `.rpmsave` 或 `.rpmnew`）。若 0.6.0 的舊旗標造成這一步仍然蓋掉檔案，在 CHANGELOG 與手冊如實寫出「從 0.6.0 升級仍會蓋掉，升級前先備份」，不要寫成已解決。完成時：結果寫進 CHANGELOG 的 Checked 或 Not verified。

## 2. 文件

- [x] 2.1 [after: 1.3] `CHANGELOG.md` 的 Unreleased 加一個條目（bug fix），`docs/manual/08-operations.md` 與 `docs/manual.zh_TW/08-operations.md` 的升級段說明 rpm 的 `emfor.toml.rpmnew` 與（若 1.3 證實）從 0.6.0 升級前先備份。完成時：`python3 tools/check-manual-links.py` 報 0 個問題。
