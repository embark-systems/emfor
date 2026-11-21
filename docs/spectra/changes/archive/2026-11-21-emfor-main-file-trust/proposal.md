## Why

主設定檔（例如 `/etc/emfor/emfor.toml`）可以宣告 `[plugins."x"]` 的 `command`，那個程式以 daemon 的權限執行；`conf.d` 的片段檔同樣可以。片段檔和 `include_dir` 目錄已經會在 group 或 others 可寫、或擁有者既不是 root 也不是執行 daemon 的使用者時被拒絕載入（spec `service-config` 的 "Config directory permissions"），主檔卻完全不檢查，而且該 requirement 明文寫著「主檔不新增權限檢查」。結果是：能寫主檔的人，等於能以 daemon 的身分執行程式，而片段檔這條路已經被堵上。這是 CASA 自我評估補強清單（TODO 67）剩下的一項。0.7.0 已經接受相容性變更（store 檔案權限、`accept_plaintext` 預設），這項放在同一版。

## What Changes

- `emfor server`（啟動與 SIGHUP 重載）和 `emfor check-config` 讀主設定檔時，套用和片段檔相同的檢查：檔案的 mode 在 group 或 others 有寫入位元（`mode & 0o022` 不為零）就拒絕；擁有者不是 root 也不是執行 emfor 的使用者（effective user）也拒絕。檢查在檔案開啟之後、對開啟的那個檔案做，並從同一個開啟的檔案讀取；符號連結以它指向的檔案判斷。
- 錯誤訊息和片段檔一樣：寫出路徑與原因（mode 或擁有者），結尾是 `refusing to load it`，並給出修正指令（`chmod g-w,o-w <path>`）。
- 啟動時拒絕：記錄錯誤、以設定錯誤的結束碼 2 結束，不打開 store。重載時拒絕：記錄錯誤、保留舊設定繼續執行（現有的重載規則）。`emfor check-config` 結束碼 2。
- `emfor status` 與 `emfor lookup` 不受影響：它們只讀主檔的 `[store]`（`load_store`），不執行 plugin，所以不檢查。主檔所在的目錄（`/etc/emfor`）不在這次的範圍內。
- 相容性變更（0.7.0）：套件安裝的主檔是 root 擁有的 0644（deb、rpm 的 conffile；FreeBSD 的 sample），不受影響；自己用 0664 建立、或由 root 與 emfor 使用者以外的帳號擁有主檔的既有安裝，升級後 daemon 起不來，直到管理者修正 mode 或擁有者。CHANGELOG 與手冊（第 3 章與升級說明，中英文）寫清楚。
- 把測試 `main_file_mode_is_not_checked`（鎖定「0664 的主檔被接受」）改成新規則的測試。

## Non-Goals

- 不檢查主檔所在的目錄（`/etc/emfor`）：deb 與 rpm 已設成 root 0755，FreeBSD 的 port 與手動安裝未必，這需要另一個決定。
- 不檢查 `emfor status` 與 `emfor lookup` 讀主檔的路徑：應用程式的使用者會以自己的帳號執行它們，擁有者是 `emfor` 的主檔會讓它們被拒絕。
- 不改 `secret_file`、`password_file`、`tls_key_file` 等其他檔案的檢查（各有自己的規則）。
- 不做「先警告、之後才拒絕」的過渡：使用者已決定直接拒絕（0.7.0 已有多項相容性變更，錯誤訊息會寫出修正指令）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: requirement "Config directory permissions" 改為也適用於主設定檔，並移除「主檔不新增權限檢查」和 scenario "Main file mode unchanged"；新增主檔被拒絕與被接受的 scenario，以及 `status`／`lookup` 不檢查的 scenario。

## Impact

- 程式：`crates/emfor-daemon/src/config.rs`（`read_layers_with` 讀主檔的方式；`read_trusted` 與 `untrusted` 共用）。`crates/emfor-daemon/src/cli.rs` 的 `load` 與 `check-config` 經由 `read_layers_with`，預期不需要改。
- 測試：`crates/emfor-daemon/src/config.rs` 的單元測試（`main_file_mode_is_not_checked` 改寫、擁有者測試）、`crates/emfor-daemon/tests/daemon.rs` 的整合測試（啟動拒絕、重載保留舊設定、status／lookup 照常）。
- 文件：`docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md`（"The main file is not checked this way" 那段）、升級說明（`docs/manual/10-migrating.md` 或第 8 章，兩種語言）、`docs/deploy/daemon.md` 與 `SECURITY.md` 若有描述主檔、`CHANGELOG.md` 的 Unreleased，以及 spec `docs/spectra/specs/service-config/spec.md`（archive 時套用）。
- 不影響：wire 格式、store 格式、client、套件內容。
