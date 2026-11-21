## Why

`[store]` 的預設值是 `file_mode = 0644`、`lock_file_mode = 0666`：本機每個使用者都能讀 store，也都能寫 lock 檔。lock 檔放著 LMDB 的 reader 表和 daemon 每次 commit 取的鎖，能寫它的人可以讓 daemon 的 commit 停住（所有 client 看到過期的 store），或讓查詢讀到正在被改寫的頁面（`examples/emfor.toml` 與 `docs/deploy/daemon.md` 已這樣寫）。手冊的建議做法是 0640／0660 加群組，但預設值沒有這樣做，要收緊得每個人自己改。TODO 67 第 12 項（CASA 自我評估的補強清單）列了這個預設值。emfor 還很新，使用者人數有限，使用者已決定破壞性的預設值變更可以接受，只要在發佈說明（CHANGELOG 的 Unreleased）與手冊寫清楚。

## What Changes

- 程式內的預設值改成 `file_mode = 0640`、`lock_file_mode = 0660`（`StoreConfig::default`）。只有 daemon 的使用者與它的群組能讀 store、寫 lock 檔；應用程式的使用者要在 daemon 的群組裡（deb 與 rpm 套件的 systemd unit 是 `Group=emfor`，deb 的 postinst 與 rpm 的 pre 腳本都會建立 `emfor` 群組），或管理者明確設回 `"0644"` 與 `"0666"`。
- **BREAKING**：daemon 每次啟動都對 store 檔與 lock 檔 `set_permissions`（`crates/emfor-daemon/src/store.rs`），所以從 0.6 升級、重啟成 0.7 的第一次啟動就會把現有檔案改成新模式。不在 daemon 群組的應用程式，在那之後的查詢會得到 `UNAVAILABLE`，直到管理者把它的使用者加進群組，或在 `[store]` 設回舊值。deb 與 rpm 套件的 `/etc/emfor/emfor.toml` 只有 `include_dir`，沒有 `[store]` 鍵，所以套件使用者是用程式內的預設值，一律受影響。
- 新增警告：`file_mode` 或 `lock_file_mode`（不論是預設值還是明確寫的）有「其他人可寫」的位元（`0002`）時，`emfor check-config`（標準錯誤）與 daemon 啟動時（`warn` 日誌）各寫一行警告；每行寫出選項與模式、這個模式讓本機任何使用者能做什麼、建議改成什麼（`lock_file_mode` 是 `"0660"`、`file_mode` 是 `"0640"`，並要把應用程式的使用者加進 daemon 的群組）、去手冊哪一節看（「Who can use the store」），讓 `check-config` 的讀者看到就知道怎麼處理；不是錯誤，不拒絕啟動，`check-config` 的結束碼不變。沿用現有 `credential_warnings` 的做法。不檢查「其他人可讀」：0644 是手冊寫明、可以接受的選擇（本機使用者都可信任的主機）。
- 文件與範例：`examples/emfor.toml`（第 21 行與第 33 行明確寫了 0644 與 0666，以及上方的說明）、手冊（英文與繁體中文）的 `[store]` 說明與「Who can use the store」（原本的建議設定變成預設值；想讓每個本機使用者都能查詢，變成明確寫 0644／0666）、`docs/deploy/daemon.md`、CHANGELOG 的 Unreleased（相容性變更，含升級步驟）。
- state-store spec 的「LMDB file layout」要求改預設值並寫明模式在每次啟動套用；新增「Store file mode warning」要求。
- 測試：`creates_files_with_configured_modes` 斷言舊預設值，要改；新增警告的單元測試與整合測試。

## Non-Goals

- 不改套件的 `/etc/emfor/emfor.toml`：deb 的 conffile 在使用者沒改過時升級會被靜默換成新版，改套件設定檔不會比改程式預設值更安全，而且改程式預設值對 FreeBSD 與手寫設定的行為一致。
- 不改 systemd unit 的 `StateDirectoryMode=0755` 與 `UMask=0027`：目錄要讓 client 走得進去，檔案模式由 `[store]` 決定。
- 不處理「lock 檔不存在時，第一個 client 會建立它（0644、擁有者是 client 的使用者）」（TODO 58 ⑥）：另一個問題，維持文件說明。
- 不加自動遷移，也不自動把使用者加進群組：只寫成文件。
- 不處理 TODO 67 第 12 項的另一半（`accept_plaintext`）：由 `emfor-accept-plaintext-default` 處理。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `state-store`: 預設的檔案模式改為 0640 與 0660，並寫明每次啟動都套用到現有檔案；新增「其他人可寫」的啟動與 check-config 警告。

## Impact

- Affected specs: `state-store`（修改「LMDB file layout」，新增「Store file mode warning」）
- Affected code:
  - Modified: `crates/emfor-daemon/src/config.rs`（`StoreConfig::default`）
  - Modified: `crates/emfor-daemon/src/store.rs`（測試 `creates_files_with_configured_modes`）
  - Modified: `crates/emfor-daemon/src/cli.rs`（新的 store 警告函式、`check-config` 與啟動的呼叫點、單元測試）
  - Modified: `crates/emfor-daemon/tests/daemon.rs`（警告的整合測試）
  - Modified: `examples/emfor.toml`、`docs/manual/03-configuration.md`、`docs/manual/08-operations.md`、`docs/manual.zh_TW/03-configuration.md`、`docs/manual.zh_TW/08-operations.md`、`docs/deploy/daemon.md`、`CHANGELOG.md`
  - New: 無
  - Removed: 無
- 相容性：行為相容性變更，0.7.0 發佈；升級步驟見 CHANGELOG。
