## 1. backend 標記：backup 與 last check failed（spec「Status text output」）

- [x] 1.1 依 spec Status text output，`emfor status` 的 backend 行在 `backup = true` 時於 pool 欄之後、`local-up` 之前顯示 `backup`；舊 store（沒有 `backup` 欄位，讀為 false）不顯示。驗證：`crates/emfor-daemon/src/status.rs` 的單元測試新增 backup 案例，涵蓋 backup 為 true、false，以及沒有該欄位的記錄（`cargo test -p emfor-daemon --lib status::`，在 Debian docker 映像 `emfor-clients-test` 內執行）。
- [x] 1.2 backend 行在 state 為 `healthy` 且 `reason` 有值時，於 `FLAPPING` 之後、reason 之前顯示黃色的 `last check failed`；`unhealthy`、`unknown`、`unrecognized` 帶 reason 時不顯示，`healthy` 沒有 reason 時也不顯示。驗證：新增單元測試，逐字比對 spec 範例「when `last check failed` is written」的五列，並確認 `NO_COLOR` 時沒有跳脫序列。 [after: 1.1]

## 2. 「投不了票」只在整個群組都投不了時，用 `peers:` 行的後綴表示

- [x] 2.1 當群組無法投票時，`peers:` 行以 `, too few live members to vote` 結尾（在 `not live` 部分之後，且不與 `checks N/M` 同時出現）。條件沿用 `assigned_checkers` 的結果：算得出 assigned checker、至少一個顯示的 backend 屬於沒被本機略過的服務、且這些 backend 都沒有 assigned checker。驗證：更新既有測試 `peers_line_table`（兩個成員都不 live 的列）、`voted_backends_table`（只有 .6 live 的列）與 `members_services_off_table`（.6、.7、.9 略過的列）的預期值，並新增 spec 的五個情境：無法投票、剛好三個候選者可投票、服務被本機略過、部分服務可投票、`listen` 無效時不顯示。 [after: 1.2]
- [x] 2.2 不改 `checked_at` 的行為，只確認並記錄：確認投票得出的結果是否更新 backend 的 `checked_at`：讀 daemon 套用結果的程式（`crates/emfor-daemon/src/schedule.rs` 的結果套用處）與其測試，把結論記在 `design.md` 的對應決策下（改一行說明即可）。若 `voted` 行的「`<age> ago`」可能讓人誤以為本機剛連線過，在中英手冊第 8 章說明，不改 daemon 行為。驗證：design.md 有明確結論，手冊敘述與程式一致（內容審閱）。

## 3. 範例與寬度

- [x] 3.1 以測試或手算量出 10 台成員、40 個 backend 的範例輸出中最長的 backend 行（含 `backup`、`last check failed`、`voted`、reason），確認 `--watch` 既有的「Output fits the terminal」截斷行為不受影響；把這個範例（含兩個新標記）放進手冊第 8 章。驗證：新增或擴充的單元測試以該範例渲染，斷言每個標記出現且行數不變；手冊範例與測試輸出一致。 [after: 2.1]

## 4. 文件

- [x] 4.1 中英手冊第 8 章（`docs/manual/08-operations.md`、`docs/manual.zh_TW/08-operations.md`）的 backend 標記清單補上 `backup`、`last check failed`、`local-up`、`voted`；`peers:` 部分說明新後綴與「只有全部可投票的服務都投不了時才顯示」；`local-up` 段落補一句指向 `emfor status --json` 的 `peers` 與 `assigned_checkers`。驗證：中英兩版的標記清單與 spec 一致，內部連結與錨點可解析（同一份標記清單、同樣的錨點）。 [after: 3.1]
- [x] 4.2 `docs/deploy/daemon.md` 的 backend 標記清單補上 `backup`、`last check failed`、`local-up`、`voted`，`--json` 欄位清單補上 `peers` 與 `assigned_checkers`（欄位已在 spec 的 JSON 情境中定義）。驗證：清單與 `status-command` spec 逐項對照（內容審閱）。 [after: 3.1]
- [x] 4.3 文字輸出變更不算不相容變更：`CHANGELOG.md` 的 Unreleased 段落記錄三項文字輸出變更與文件補充，並說明文字輸出不屬穩定介面、`--json` 與結束碼未變。驗證：條目與 spec delta 一致，且不宣稱已實測的事（內容審閱）。 [after: 4.1]

## 5. 驗證

- [x] 5.1 在 Debian docker 映像 `emfor-clients-test`（必要時用 `docker build -q -t emfor-clients-test tests/clients` 重建）執行 `cargo test --workspace`、`cargo clippy --workspace --all-targets` 與 `cargo fmt --check`；執行 `spectra validate emfor-status-clarity`。驗證：全部通過；沒有跑的檢查（macOS、FreeBSD）在回報中明說未測。 [after: 4.3]
