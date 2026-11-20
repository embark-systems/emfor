## 1. read_secret 的擁有者檢查（Peer secret file owner）

- [x] 1.1 `crates/emfor-daemon/src/config.rs` 的 `read_secret` 拒絕「開啟的檔案（跟著連結）擁有者既不是 uid 0 也不是有效 uid」的 secret 檔，訊息為「<path> is owned by uid N, not root or the user running emfor」；檢查放在既有權限檢查旁，從同一份 metadata 取擁有者，不再依檔名查第二次。判斷是一個吃檔案 uid 與有效 uid 的純函式（比照 `untrusted()`），由單元測試驗證 spec「Peer secret file owner」範例表的五列：(0, 998) 接受、(998, 998) 接受、(1000, 998) 拒絕、(0, 0) 接受、(1000, 0) 拒絕。
- [x] 1.2 [after: 1.1] 擁有者錯誤經由 `read_secret` 既有的呼叫端（設定驗證與 `peers/task.rs`）在啟動、`emfor check-config` 與 reload 時出現在 `peers.secret_file` 與 `peers.previous_secret_file`；用 `config.rs` 的測試驗證：擁有者是測試自己的使用者的 secret 檔（權限 0600 與 0640）仍被接受；若測試以 root 執行，chown 給其他 uid 的檔案被拒絕，否則由 `tools/docker-test.sh` 在 Debian 上跑測試。

## 2. 文件

- [x] 2.1 `docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md` 的 `secret_file` 列說明檔案必須屬於 root 或執行 emfor 的使用者（群組不論）；讀兩列內容並跑 `spectra validate` 驗證。
- [x] 2.2 `CHANGELOG.md` 的 Unreleased 有一則條目，說明別的帳號擁有的 peer secret 現在在啟動、`check-config` 與 reload 被拒絕，以及怎麼修（`chown` 給 root 或 emfor 使用者，群組不變），標明為 0.7.0 的相容性變更；讀條目驗證。

## 3. 驗證

- [x] 3.1 [after: 1.2, 2.2] `tools/docker-test.sh`（fmt、clippy 警告視為錯誤、全部測試）在 Debian 上通過，且 `spectra validate emfor-secret-owner` 與 `spectra analyze emfor-secret-owner` 沒有 Critical 或 Warning。
