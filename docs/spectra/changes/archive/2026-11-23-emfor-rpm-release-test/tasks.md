## 1. 測試腳本

- [x] 1.1 新增 `tools/rpm-release-test.sh`：啟 `rockylinux:9`（systemd 當 PID 1）、安裝 rpm、檢查使用者與群組、預設 disabled、enable --now、reload、`emfor status --json`、stop、`rpm -V`、`rpm -e` 後 `.rpmsave` 與資料保留、重新安裝；每項印 `ok:`／`FAIL:`，失敗以非零結束，結束時移除容器；驗證：用本機 arm64 build.sh 建出的 rpm 跑一次，輸出 `all checks passed`，並故意傳不存在的 rpm 確認印用法且非零結束。
- [x] 1.2 [after: 1.1] 腳本加入升級段落（接受第二個舊版 rpm）：檢查 MainPID 在升級後改變且仍 active、使用者設定檔修改保留、`emfor status --json` history 保留、`rpm -U --oldpackage` 降版後能啟動；沒給舊版 rpm 時印 `upgrade checks skipped`；驗證：用上一個 tag（git worktree 內 `packages/build.sh`）建出的 rpm 與目前 rpm 各跑一次升級與降版，輸出 `all checks passed`。

## 2. 文件

- [x] 2.1 [after: 1.2] RELEASING.md 加「發版前跑 rpm 測試」步驟：怎麼用 `git worktree` 加 `packages/build.sh` 建上一版 rpm、腳本的指令、建議在 x86-64 機器與 Rocky VM 各跑一次、容器不等於真實主機；驗證：內容審閱，指令與 1.2 實跑的一致。
- [x] 2.2 [after: 1.2] packages/README.md 的「Not tested」與 docs/manual/04-installing.md、docs/manual.zh_TW/04-installing.md 的「已測／未測」改成與實際跑過的一致（寫明架構、容器內 systemd、未測 SELinux enforcing 與真實主機），並寫明 RHEL 8 不支援（`usr/bin/emfor` 需要 GLIBC_2.30，RHEL 8 為 2.28），只支援 RHEL 9 以上；驗證：內容審閱對照 1.2 實際輸出，`python3 tools/check-manual-links.py` 通過。
- [x] 2.3 [after: 2.2] CHANGELOG.md 的 Unreleased 記一筆（新增 rpm 發版前測試腳本、RHEL 8 不支援的說明）；驗證：內容審閱。

## 3. 收尾

- [x] 3.1 [after: 2.3] 用 tools/docker-test.sh 確認 repo 內既有測試仍通過（本 change 不改程式碼，預期無影響）；驗證：docker-test.sh 回報全部通過。
