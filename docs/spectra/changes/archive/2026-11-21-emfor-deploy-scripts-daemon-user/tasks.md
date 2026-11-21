## 1. 腳本

- [x] 1.1 `examples/deploy/emfor-peers-sync.sh` 與 `examples/deploy/emfor-drain.sh`：加入 `EMFOR_USER`（預設 `emfor`）、`quote` 與 `as_daemon_user` 函式，`check-config` 改經 `as_daemon_user` 執行（root 且使用者不是 root 時用 `runuser -u`，否則 `su -m ... -c` 搭配單引號參數；其他情況直接執行）。完成時：`sh -n` 通過，兩份腳本在 `check-config` 以外的流程不變，檔頭註解說明 `EMFOR_USER`。
- [x] 1.2 [after: 1.1] 在 Debian 容器以 root 實測：(a) 設定宣告一個記錄自己 uid 的 plugin，經 `emfor-peers-sync.sh` 跑 `check-config` 時 plugin 的 uid 是 `emfor` 的，不是 0；`EMFOR_USER=root` 時是 0；(b) 沒有 plugin 時，成功寫入新名單與「check-config 拒絕時還原檔案」兩條路徑照舊；(c) `emfor-drain.sh on` 同樣以 `emfor` 跑 check-config；(d) `EMFOR_USER` 指到不存在的使用者時，腳本還原檔案並以錯誤結束。完成時：記錄存到 scratchpad，結果寫進 CHANGELOG。`su -m` 這一條路徑只在 Debian 容器模擬（把 `runuser` 從 PATH 拿掉），FreeBSD 沒有實測。

## 2. 文件

- [x] 2.1 [after: 1.2] `docs/manual/08-operations.md` 與 `docs/manual.zh_TW/08-operations.md` 中兩份腳本的說明（以 root 執行；check-config 以 daemon 的使用者執行；`EMFOR_USER`）；`CHANGELOG.md` 的 Unreleased 加條目（含 Checked 與 Not verified）。完成時：`python3 tools/check-manual-links.py` 報 0 個問題。
