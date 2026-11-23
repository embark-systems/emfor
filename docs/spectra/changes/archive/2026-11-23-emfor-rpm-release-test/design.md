## Context

- rpm 由 cargo-generate-rpm 依 `crates/emfor-daemon/Cargo.toml` 的 `[package.metadata.generate-rpm]` 產生，用 `packages/build.sh`（`PLATFORM` 環境變數、`packages/Dockerfile`、glibc 2.31 的 rust:1-bullseye）建出，輸出在 `packages/dist/`。
- scriptlet 行為（packages/rpm/{pre,post,preun,postun}.sh）：pre 建 emfor 群組與使用者；post 設 /etc/emfor 的擁有者與權限、有 `/run/systemd/system` 時 daemon-reload，不 enable；preun 在最後一次移除時 `disable --now`；postun daemon-reload，`$1 -ge 1`（升級）時 try-restart。
- `/etc/emfor/emfor.toml` 與 `/etc/emfor/conf.d/README` 是 `%config(noreplace)`。套件內的 emfor.toml 只有 `include_dir = "conf.d"`，沒有服務，所以要啟動並看到 history，必須先放一個 conf.d 設定檔（例如 exec 檢查 `/bin/true`）。
- 之前的測試是手動、一次性的（Rocky 9.9 arm64 容器，systemd 252 當 PID 1，`--privileged`）。`rockylinux:9` 映像沒有 systemd，要用 dnf 裝。
- 本機是 arm64 的 Mac，只能在容器內跑 arm64 的 rpm；x86-64 要換機器或 VM 跑同一支腳本。

## Goals / Non-Goals

Goals:

- 一支可重跑的腳本，檢查 rpm 在 systemd 下的安裝、服務生命週期、升級、降版、驗證、移除重裝。
- 文件只寫實際跑過的範圍；RHEL 8 不支援寫清楚原因。

Non-Goals:

- 不改程式碼、unit、scriptlet、套件內容。
- 不測 SELinux enforcing、不建 RHEL 8 的 rpm、不加 CI job、不簽章、不發布。
- 不把容器內的 systemd 說成真實主機。

## Decisions

**腳本的結構：外層啟容器，內層在容器裡跑檢查。** 同一個檔案用參數分兩個入口：`tools/rpm-release-test.sh <new.rpm> [<old.rpm>]` 在主機上執行，啟動 `rockylinux:9`（`--privileged`，systemd 當 PID 1，掛載 rpm 目錄為唯讀），再用 `docker exec` 跑自己的內層段落。原因：沿用 tools/sign-packages.sh 的「主機只需要 docker」的用法，不需要另一個檔案。替代方案：另寫 Dockerfile 預裝 systemd，因為多一個要維護的映像而不採用；改在容器啟動時用 dnf 裝 systemd，與 TODO 第 24 項的做法一致。

**不 enable 的預設要被檢查。** 安裝後 `systemctl is-enabled emfor` 必須是 `disabled`，因為 post 不 enable；之後腳本才手動 `enable --now`。

**升級資料用 conf.d 檔與修改過的 emfor.toml 驗證。** 舊版安裝後，腳本寫入 `conf.d/test.toml`（exec `/bin/true`）、在 `/etc/emfor/emfor.toml` 附加一行註解以模擬管理者修改；升級後確認該行仍在、`/etc/emfor/emfor.toml.rpmnew` 存在（僅在新舊套件的設定檔內容不同時才會產生，若兩版相同，腳本改為檢查檔案內容未被改動並印出說明，不判失敗）。

**daemon 被重啟的證據用 MainPID 變化。** 升級前後比較 `systemctl show -p MainPID emfor`，必須不同且服務仍是 active。history 保留的證據：升級前 `emfor status --json` 有該服務的歷史紀錄，升級後仍然有（store 在 `/var/lib/emfor`，StateDirectory 不隨升級刪除）。

**降版用 `rpm -U --oldpackage`。** 目的是確認舊版 daemon 讀得了新版寫過的 store（0.7.x 沒有 store 版本遷移，每次啟動重建 store），啟動後 `is-active` 為 active。

**上一版 rpm 不放進 repo。** RELEASING.md 描述：`git worktree add ../emfor-prev <上一個 tag>`，在該 worktree 內執行 `packages/build.sh`，取其 `packages/dist/` 的 rpm 當第二個參數。理由：沒有發布過 rpm，且 build.sh 會建它所在的 checkout。

**沒有舊版參數時略過升級段落並明說。** 腳本只給一個 rpm 時只做單版檢查，結尾印出「upgrade checks skipped」，不當作通過。

**文件寫法：只寫跑過的。** 「已測」列出：Rocky 9 容器（systemd 當 PID 1）下的安裝、服務生命週期、升級、降版、`rpm -V`、移除重裝，附上架構（實際跑過哪些）。「未測」列出：SELinux enforcing、真實主機（容器不算）、RHEL 8（不支援）、其他發行版；x86-64 只有在實際跑過後才從「未測」移到「已測」。

## Implementation Contract

- 腳本用法：`tools/rpm-release-test.sh <new.rpm> [<old.rpm>]`；缺參數或檔案不存在時印用法並以非零結束；環境變數 `ROCKY_IMAGE` 預設 `rockylinux:9`。
- 腳本需要 docker；結束時一定移除它啟的容器（包含失敗時）。
- 每個檢查印一行 `ok: <說明>`；任何檢查失敗印 `FAIL: <說明>` 並以非零結束；全部通過時印 `all checks passed` 並以 0 結束。
- 檢查項目：安裝成功；`emfor` 使用者與群組存在；服務預設 disabled；放入 conf.d 設定後 enable --now 為 active；`systemctl reload emfor` 後仍 active；`emfor status --json` 回傳該服務；stop 後 inactive；`rpm -V emfor` 無輸出（以 `-V` 回傳 0，已知被管理者改過的設定檔除外，改過的檔案在驗證前還原或明確排除）；`rpm -e` 後 binary 與 unit 消失、`/etc/emfor` 內被修改的檔案以 `.rpmsave` 保留、使用者與資料目錄保留；重新安裝成功。
- 升級段落（有 `<old.rpm>` 時）：見 Decisions。
- 不修改 repo 內任何其他檔案；不放進 tools/docker-test.sh；CI 不呼叫它。
- 範圍內：腳本、RELEASING.md、packages/README.md、手冊第 4 章（英文與 zh_TW）、CHANGELOG.md。範圍外：見 Non-Goals。
- capability 層級的可觀察行為沒有改變。

驗證方式：`tools/rpm-release-test.sh packages/dist/emfor-<版本>-1.<arch>.rpm [舊版 rpm]`（需要 docker，在本機 arm64 與一台 x86-64 機器上各跑）。這支腳本自己啟動容器，與 tools/docker-test.sh 無關；文件檢查用 `python3 tools/check-manual-links.py`。

## Risks / Trade-offs

- [容器內的 systemd 在不同 Docker 主機（macOS Docker Desktop、Linux、cgroup v1/v2）行為不一] -> 腳本用 `--privileged --cgroupns=host` 並在 task 1.1 先實際跑通；跑不通的環境在 RELEASING.md 註明。
- [x86-64 在本機測不到] -> 文件只有在有人實際跑過之後才把 x86-64 標為已測；RELEASING.md 建議發版前在 x86-64 機器跑一次。
- [系統行為判斷依賴 rpm 版本，例如 `.rpmnew`、`.rpmsave` 的產生條件] -> 腳本把「新舊設定檔內容相同」視為不適用並明說，不誤判失敗。
- [沒有真實主機、沒有 SELinux] -> 文件明寫容器不等於真實主機。

## Migration Plan

純新增工具與文件修改，沒有部署步驟。回滾：還原這個 commit 即可。

## Open Questions

None
