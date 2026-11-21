## Why

`examples/deploy/emfor-peers-sync.sh` 與 `emfor-drain.sh` 都以 root 執行，並在改檔後以 root 跑 `emfor check-config`。`check-config` 載入設定時會啟動設定裡宣告的 plugin（`crates/emfor-daemon/src/cli.rs` 的 `start` 在 `check_only` 時先 `load` 再 `plugins.commit`），所以 plugin 會以 root 在 systemd 的沙箱外執行；daemon 本身是以 `emfor` 使用者在硬化的 unit 內執行 plugin。plugin 的執行檔或其目錄若可被非 root 寫入，就是提權到 root。FreeBSD 的 rc 腳本（`packages/freebsd/files/emfor.in`）已為同一理由用 `su -m` 以 `emfor_user` 跑 `check-config`；兩份範例腳本與手冊（`docs/manual/08-operations.md` 要求「以 root 執行」）沒有。這是 2026-11-21 的 Fable 審查的項目，其中 `emfor-peers-sync.sh` 是這個 session 前面寫的。

## What Changes

- 兩份腳本新增 `EMFOR_USER`（預設 `emfor`）與一個函式，在以 root 執行、且 `EMFOR_USER` 不是 `root` 時，以該使用者執行 `emfor check-config`：有 `runuser` 就用 `runuser -u "$EMFOR_USER" --`，否則用 `su -m "$EMFOR_USER" -c`（FreeBSD 的做法，參數以單引號引起來）；不是 root 或使用者是 `root` 時直接執行。`status --json`（只讀 store）與 reload 不變。
- 檔頭註解與 `docs/manual/08-operations.md`（與 zh_TW）說明：check-config 以 daemon 的使用者執行，因此 conf.d 檔案要能被該使用者讀（本來就如此，daemon 也要讀），以及 `EMFOR_USER`。
- 在 Debian 容器以 root 實測：設定裡有一個會記下自己 uid 的 plugin 時，check-config 啟動的 plugin 不是 uid 0；沒有 plugin 時兩份腳本的原有流程（成功寫入、拒絕時還原）照舊。

## Non-Goals (optional)

- 不處理 `emfor-drain.sh` 與 `emfor-peers-sync.sh` 先 `mv` 上線再 `check-config` 的順序問題（Fable 審查的另一項，低風險；`check-config` 不能指向一份候選檔，要改就得改 emfor 本身）。
- 不改 `emfor check-config` 本身（例如拒絕以 root 啟動 plugin）：那是行為變更，需要另開變更。
- 不改 `examples/pf/emfor-pf-table.sh`。
- 沒有 design.md：做法小，上面已寫出取捨。

## Impact

- Affected specs: none
- Affected code:
  - New: (none)
  - Modified: `examples/deploy/emfor-peers-sync.sh`、`examples/deploy/emfor-drain.sh`、`docs/manual/08-operations.md`、`docs/manual.zh_TW/08-operations.md`、`CHANGELOG.md`
  - Removed: (none)
- Compatibility: 腳本仍以 root 執行；`check-config` 現在以 `emfor`（或 `EMFOR_USER`）執行，所以該使用者不存在、或讀不到設定與密鑰檔時，`check-config` 失敗，腳本照原有流程還原檔案並結束（不會留下壞設定）。
