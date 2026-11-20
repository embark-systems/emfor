## 1. 措辭修正（Peer secret file owner）

- [x] 1.1 讀 `crates/emfor-daemon/src/config.rs` 的 `read_secret`、`owner_trusted` 與測試 `peer_secret_owner_is_root_or_the_daemon_user`，確認 delta spec「Peer secret file owner」所寫的行為與程式一致：`read_secret` 以做檢查的行程自己的 `geteuid()` 比對，錯誤訊息仍是「owned by uid N, not root or the user running emfor」，範例表的五列與測試的五列相同；驗證：內容審閱，並以 `git diff --stat -- crates` 確認這個 change 沒有動任何程式或測試。
- [x] 1.2 [after: 1.1] delta spec 的要求本文、情境與範例表，把檢查者寫成「做這次檢查的行程」（daemon 在啟動與 reload，`emfor check-config` 時是執行它的使用者），並含兩個 `emfor check-config` 由非 root 帳號執行的情境（daemon 使用者擁有的 secret 被拒絕、root 擁有的被接受）；驗證：讀 `docs/spectra/changes/emfor-secret-owner-wording/specs/peer-group/spec.md` 並執行 `spectra validate emfor-secret-owner-wording`。

## 2. 確認其他文件不用改

- [x] 2.1 [after: 1.2] `CHANGELOG.md` 的 Unreleased 條目與 `docs/manual/03-configuration.md`、`docs/manual.zh_TW/03-configuration.md` 的 `secret_file` 列沒有把檢查者寫成 daemon；驗證：讀這三處並以 `grep -n "effective user the daemon" CHANGELOG.md docs/manual docs/manual.zh_TW` 確認沒有符合的行，這個 change 不編輯這三個檔案。

## 3. 驗證

- [x] 3.1 [after: 2.1] `spectra validate emfor-secret-owner-wording` 通過，`spectra analyze emfor-secret-owner-wording` 沒有 Critical 或 Warning；驗證：執行這兩個指令並讀輸出。

<!-- 變更說明（非 task）：原 3.1 的「封存後 live spec 不再含舊句」要等封存才能驗證，封存後這個 change 已移走，task 無法再標記完成，所以不再是 task。封存後、commit 前由人檢查 `grep -n "the effective user the daemon runs as" docs/spectra/specs/peer-group/spec.md` 沒有符合的行，並把結果寫進 commit 訊息的 Checked 段。 -->
