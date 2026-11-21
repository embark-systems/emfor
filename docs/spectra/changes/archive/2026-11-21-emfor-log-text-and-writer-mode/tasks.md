## 1. 日誌文字

- [x] 1.1 實作設計決定「跳脫沿用既有規則」：`crates/emfor-daemon/src/checker/mod.rs` 新增 `escape_unprintable` 並讓 `normalize_reason` 使用它；`crates/emfor-daemon/src/plugin/host.rs` 的 `log_stderr` 與啟動日誌（名稱、版本）寫入前先跳脫。完成時：新測試顯示含 `\r` 與 U+202E 的 stderr 行、以及含這些字元的 Hello 名稱與版本，在日誌裡是跳脫後的文字；`normalize_reason` 既有測試不變地通過。

## 2. redis

- [x] 2.1 實作設計決定「redis 用字串取代遮蔽密碼」：`crates/emfor-check/src/redis.rs` 的 `auth: {e}` 與 `describe` 路徑在回報前把密碼換成 `***`。完成時：新測試顯示伺服器回 `-ERR bad password hunter2` 時失敗原因不含 `hunter2`（用測試檔現有的假伺服器；若沒有就用 `redact` 的單元測試並說明）。

## 3. 寫入端鎖檔

- [x] 3.1 實作 spec state-store "Single writer per store" 的模式（設計決定「`.writer` 的模式」）：`crates/emfor-daemon/src/store.rs` 的 `Store::open`。完成時：新測試顯示新建的與原本 0644 的 `<path>.writer` 在 `Store::open` 之後是 0600，且 chmod 失敗路徑不讓 `open` 失敗（以註解說明如何驗證，若無法在測試裡造出失敗就說明未驗證）。

## 4. 文件

- [x] 4.1 [after: 1.1, 2.1, 3.1] `docs/manual/01-concepts.md` 與 `docs/manual.zh_TW/01-concepts.md` 的 `<store>.writer` 一列加上模式 0600；`CHANGELOG.md` Unreleased 加條目（Checked／Not verified）。完成時：`python3 tools/check-manual-links.py` 報 0 個問題，`spectra validate` 通過。
