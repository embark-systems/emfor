## Context

`config.rs` 的 `read_layers_with` 是 `emfor server`（啟動與 SIGHUP 重載，經 `cli.rs` 的 `load`）和 `emfor check-config` 讀設定的入口。它用 `read_table(path)` 讀主檔：直接 `read_to_string`，不看 mode 與擁有者。同一個函式讀片段檔時用 `read_trusted`：先開檔，對開啟的那個檔案取 metadata，用 `untrusted(mode, uid, euid)` 判斷（`mode & 0o022` 不為零就拒絕；擁有者不是 root 也不是 euid 就拒絕），再從同一個檔案讀取。`include_dir` 目錄本身也用同一個 `untrusted` 檢查。理由是片段檔可以宣告 `[plugins."x"]` 的 `command`，那個程式以 daemon 的權限執行；主檔可以宣告同樣的東西，卻不檢查。

`load_store`（`emfor status`、`emfor lookup`）另外用 `read_table` 只讀主檔的 `[store]`，不執行 plugin。測試 `main_file_mode_is_not_checked` 鎖定「mode 0664 的主檔被接受」，spec `service-config` 的 "Config directory permissions" 明文寫「主檔不新增權限檢查」。

已讀過的套件事實：0.6.0 的 deb 與 rpm 把 `/etc/emfor/emfor.toml` 裝成 root 擁有、mode 0644（`dist/` 裡的套件檔案內容）；FreeBSD 的 port 用 `INSTALL_DATA`（0644）裝 `emfor.toml.sample`；systemd unit 的 `ExecStartPre` 以 `User=emfor` 執行 `emfor check-config`，主檔擁有者是 root，通過。

## Goals / Non-Goals

**Goals:**

- `emfor server`（啟動與重載）與 `emfor check-config` 拒絕 group 或 others 可寫、或擁有者既非 root 也非 euid 的主檔，規則和片段檔相同。
- 檢查在開啟的檔案上做，並從同一個開啟的檔案讀取，符號連結以指向的檔案判斷。
- 錯誤訊息寫出路徑、原因與修正指令。

**Non-Goals:**

- 不檢查主檔所在的目錄（`/etc/emfor`）。
- 不檢查 `emfor status`、`emfor lookup` 讀主檔的路徑（`load_store`）。
- 不改其他檔案（`secret_file`、`password_file`、`tls_key_file`）的檢查。
- 不做警告期；不加設定項目讓管理者關掉檢查。

## Decisions

### 拒絕，不是警告

使用者決定（2026-11-21）：直接拒絕，作為 0.7.0 的相容性變更。理由：主檔和片段檔一樣能以 daemon 的身分執行程式，片段檔已經拒絕，主檔只警告會留下一條較弱的路；0.7.0 已有多項相容性變更，錯誤訊息會寫出修正指令；套件安裝的主檔是 root 0644，不受影響。代價：用 0664 或由別的帳號擁有主檔的既有安裝，升級後 daemon 起不來，直到修正。

### 共用 `read_trusted`，只換 `read_layers_with` 讀主檔的那一行

`read_layers_with` 讀主檔改用 `read_trusted`（回傳錯誤的格式和片段檔相同：`<path>: <reason>`）。`load_store` 與 `read_table` 不動，所以 `status` 和 `lookup` 不檢查。無法開啟或讀取主檔的訊息（`cannot read <path>: <OS 錯誤>`）現在由 `read_table` 產生，`read_trusted` 的同類訊息格式相同，所以這類錯誤的文字維持不變；我只確認過 `config.rs` 與 `tests/daemon.rs` 裡 `cannot read` 的斷言都是針對 secret 檔，沒有針對主檔的，所以沒有現有測試鎖定它，實作時要補一個（不存在的主檔仍得到 `cannot read`）。

### 錯誤訊息附修正指令

`untrusted` 的原因文字對 mode 附 `chmod g-w,o-w <path>`，對擁有者附 `chown root <path>`。這也會讓片段檔的訊息多這段文字；現有測試對片段檔的斷言用子字串（路徑、`0664`、`owned by uid 12345`），不受影響。

### 錯誤歸屬與重載

主檔被拒絕是整個載入的錯誤：啟動時記錄後以結束碼 2 結束，不打開 store；重載時記錄後保留舊設定（現有的 "Any error in the main file or any fragment SHALL reject the whole load"）。不需要新機制。

## Implementation Contract

- **Behavior**：`emfor check-config --config <main>` 對 mode `& 0o022` 不為零的主檔印出 `<path>: writable by group or others (mode 0664); refusing to load it` 及修正指令，結束碼 2；對擁有者既非 root 也非 euid 的主檔印出 `<path>: owned by uid N, not root or the user running emfor (uid M); refusing to load it` 及修正指令，結束碼 2。`emfor server` 啟動時同樣拒絕，結束碼 2，不打開 store。重載時（SIGHUP）把主檔改成 0664：記錄錯誤與 `reload rejected; keeping the previous config`，群組與服務照舊。`emfor status`、`emfor lookup` 在 0664 的主檔下照常運作。
- **Interface**：不新增設定項目、指令或選項。`read_layers_with` 與 `read_layers` 的簽章不變。
- **Failure modes**：主檔無法開啟或讀取、TOML 語法錯誤的訊息維持現狀；mode 或擁有者檢查失敗時，不繼續讀取內容。
- **Acceptance**：單元測試（`config.rs`）：主檔 mode 0664、0666、0620、0602 被拒絕；0644、0640、0600、0604 被接受；擁有者為 uid 12345 的主檔被拒絕（只在 root 時執行，同 `foreign_owner_is_refused`）；指向 0664 檔案的符號連結被拒絕。整合測試（`tests/daemon.rs`）：daemon 以 0664 主檔啟動時結束碼為 2 且 store 沒有被建立；執行中把主檔改為 0664 並送 SIGHUP，舊設定保留；`emfor status` 與 `emfor lookup` 在 0664 主檔下成功。`emfor check-config` 結束碼 2 與訊息的測試。整個 workspace 的測試與 clippy 在 Debian 容器通過（`tools/docker-test.sh`）。
- **Scope**：範圍內：`read_layers_with` 讀主檔、`untrusted` 的訊息、測試、文件、spec、CHANGELOG。範圍外：主檔所在目錄、`load_store`、其他檔案的權限檢查、套件內容。

## Risks / Trade-offs

- 既有安裝升級後起不來：用 0664 自建主檔，或主檔由 root 與 emfor 使用者以外的帳號擁有。緩解：錯誤訊息寫出修正指令；`systemctl reload` 與 `service emfor reload` 先跑 `check-config`；CHANGELOG 與手冊寫明。套件安裝的不受影響（見 Context）。
- 以 root 手動執行 `emfor check-config`，而主檔由 `emfor` 使用者擁有時會被拒絕（`owner_trusted` 只認 root 與 euid），和片段檔現有的行為相同，手冊已有這段說明，要涵蓋主檔。
- 現有測試或測試輔助函式建立的主檔如果 mode 較寬或擁有者特殊，會因新檢查失敗；實作時跑完整測試才知道，沒有事先確認。

## Open Questions

（無；使用者已決定拒絕與範圍。）
