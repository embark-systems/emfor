## Why

TODO 73 第 8 項：emfor 的 rpm 目前只在 Rocky 9.9 arm64 容器裡以 systemd 當 PID 1 手動跑過一次（CHANGELOG 與 TODO 第 24 項有記錄）。x86-64 的 rpm、版本之間的 `rpm -U` 升級與降版、`rpm -V`、移除後重裝都沒測過，也沒有可重跑的腳本。packages/README.md 寫「Not tested: the rpm under systemd」，手冊第 4 章寫「as a long-running service on a real host」，兩邊說法不一致，也都沒有對應實際跑過的內容。

另外 RHEL 8 實際上跑不了：0.7.0 arm64 rpm 裡的 `usr/bin/emfor` 最高用到 `GLIBC_2.30`，RHEL 8 的 glibc 是 2.28。這件事目前沒有寫在任何文件，使用者只會在安裝後才發現。

## What Changes

- tools/ 新增一支 committed 腳本（沿用 tools/sign-packages.sh 的風格），在 `rockylinux:9` 容器內以 systemd 當 PID 1 安裝指定的 rpm，檢查 enable、start、reload、stop、erase。升級檢查：先裝上一版 rpm，再 `rpm -U` 到目前版本，確認 emfor 帳號與群組還在、被改過的 `/etc/emfor/emfor.toml` 沒被覆蓋（產生 `.rpmnew`）、daemon 被 try-restart、`emfor status --json` 的 history 在重啟後保留；再降版（`rpm -U --oldpackage`）確認能啟動；最後 `rpm -V` 乾淨、移除後重裝。同一支腳本可在 arm64 與 x86-64 的 Docker 上跑，不放進 tools/docker-test.sh。
- RELEASING.md 加「發版前跑」步驟：上一版 rpm 怎麼用 `git worktree` 加 `packages/build.sh` 現建，並建議在 x86-64 機器與 Rocky VM 上各跑一次。
- packages/README.md 的「Not tested」與 docs/manual/04-installing.md 的「What has been tested」（含 zh_TW）改成與實際跑過的內容一致；沒真的跑過的不寫成測過。
- 手冊第 4 章、packages/README.md 與 zh_TW 寫明 RHEL 8 不支援（原因：glibc 2.28 < 2.30），只支援 RHEL 9 以上。
- CHANGELOG.md 的 Unreleased 記一筆。

## Non-Goals

- 不為 RHEL 8 重新建置 rpm（要換較舊 glibc 的建置映像，另案評估）。
- 不測 SELinux enforcing：容器測不到，只寫文件（TODO 73 第 7 項已決定）。
- 不加 CI job：CI 全在 x86-64 ubuntu-latest 上，且不建 rpm；這支腳本留給發版前手動跑。
- 不做簽章、不發布 rpm。
- 不改任何程式碼（daemon、unit、scriptlet 都不動）；若腳本跑出問題，另開 change 修。
- 容器裡的 systemd 不等於真實主機，文件不能把它寫成「在真實主機上測過」。

## Impact

- Affected specs: none
- Affected code:
  - New: tools/rpm-release-test.sh
  - Modified: RELEASING.md, packages/README.md, docs/manual/04-installing.md, docs/manual.zh_TW/04-installing.md, CHANGELOG.md
  - Removed: (none)
- Compatibility: no capability-level observable behavior changes
