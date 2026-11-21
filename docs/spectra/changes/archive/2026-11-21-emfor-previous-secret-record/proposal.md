## Why

`previous_secret_file` 讓重啟中的 daemon 在換密鑰期間繼續簽名與接受舊密鑰。現在 `start_keys`（`crates/emfor-daemon/src/peers/task.rs`）與 `Keys::resumed`（`crates/emfor-daemon/src/peers/crypto.rs`）只看兩件事：`previous_secret_file` 的內容不同於現行密鑰，以及 `secret_file` 的修改時間在 120 秒內。它們不知道那個舊密鑰是不是 daemon 真的換掉的那一把。

手冊（`docs/manual/03-configuration.md`）說 `previous_secret_file` 「可以一直留在設定裡」。於是下一次輪換（K1→K2）只更新 `secret_file`、previous 還是 K0 時，120 秒內重啟的 daemon 會把已退役的 K0 當成舊密鑰：簽 60 秒、收 120 秒。結果是用退役（可能已外洩）的密鑰簽名，而其他已丟掉 K0 的成員收不到這台的訊息。`touch` 或從備份還原 `secret_file` 也會觸發。影響有上限（簽 60 秒、收 120 秒），這是 2026-11-21 Fable 審查的一個中等項目。

## What Changes

- daemon 在 `<store.path>-peers`（模式 0600）記錄自己用過的密鑰：現行密鑰的 SHA-256 指紋，以及它換掉的密鑰的指紋與換掉的時間。只存指紋，不存密鑰。啟動時與每次密鑰變更時寫入。
- 啟動時，只有 `previous_secret_file` 的內容正好是記錄裡「被換掉的那一把」、而且那次變更在 120 秒內，才照現在的方式使用它。啟動時發現現行密鑰與記錄不同（daemon 停機時換了檔），把記錄裡的現行密鑰視為被換掉的，變更時間取 `secret_file` 的修改時間。
- 沒有記錄（例如升級後第一次啟動）：忽略 `previous_secret_file`，記一行 `info`，並寫入記錄。這是使用者已決定的做法。
- 手冊、spec（peer-group 的 "Peer secret reload"）與 CHANGELOG 對應更新；手冊說明升級後第一次啟動、以及記錄檔的位置。

## Non-Goals (optional)

- 不改簽名與接受舊密鑰的時間（60／120 秒），不改執行中的輪換行為。
- 不新增設定項目；記錄檔路徑由 `store.path` 推得。
- 不把記錄放進 LMDB store：store 每次啟動重建，而且寫入端 `writer` 是 `Option`。
- 不處理多次連續輪換（K0→K1→K2 在 60 秒內）重啟後只記得最後一次替換，這和現有行為相同。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `peer-group`: "Peer secret reload" 增加記錄檔與啟動時採用 `previous_secret_file` 的條件，新增四個情境。

## Impact

- Affected specs: peer-group
- Affected code:
  - New: (none)
  - Modified: `crates/emfor-daemon/src/peers/task.rs`、`crates/emfor-daemon/src/peers/crypto.rs`、`crates/emfor-daemon/src/cli.rs`（把 store 路徑傳給 `peers::start`）、`docs/manual/03-configuration.md`（與 zh_TW）、`CHANGELOG.md`
  - Removed: (none)
- Compatibility: 升級後第一次啟動若剛好在換密鑰的 120 秒內，`previous_secret_file` 不會被採用（和今天沒設定它一樣）；之後正常。`store.path` 所在目錄要讓 daemon 能建檔（store 本來就要）；寫不出來時只警告，daemon 照常執行，但下次啟動沒有記錄，`previous_secret_file` 就會被忽略。
- 順序：這個 change 的 delta 建立在 `emfor-secret-read-timeout` 的 "Peer secret reload" 版本上，且改同一個函式所在的檔案（`peers/task.rs`）。要先 archive `emfor-secret-read-timeout`（它尚未 archive、尚未 commit），再 apply 這個 change，並在 apply 前確認 delta 仍以 archive 後的 spec 為底。
