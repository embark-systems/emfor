## 1. 寫小節

- [x] 1.1 在 `docs/manual/05-configuring.md` 的「Generating the config」之後加一個新小節（英文），寫五點：用 reload 不用 restart、驗證用 daemon 的使用者跑 `emfor check-config`、工具內建的驗證選項不能直接用（`check-config` 讀主設定檔和 `include_dir`，沒有只檢查一個片段的參數；改用 `emfor --config <暫存主檔> check-config` 檢查整份暫存設定目錄，或像 `examples/deploy/emfor-drain.sh` 先寫、檢查、失敗放回舊檔）、設定檔與 peer secret 的擁有者與權限（暫存檔也要符合）、deb 與 rpm 沒有發佈在任何套件庫。每一點最多附一行 Ansible、Puppet、Salt 各自的設定名稱，只用 proposal 列出的已查證名稱。事實與同一章、第 4、11、12、13 章一致，並連到那些章節。驗證：agent 逐點對照 proposal 與上述章節的原文，沒有新增未查證的名稱或數字；`tools/check-manual-links.py` 在任務 2.2 一起跑。
- [x] 1.2 [after: 1.1] 在 `docs/manual.zh_TW/05-configuring.md` 的對應位置加繁體中文版：事實、指令、設定名稱、連結與英文版逐項一致，用語照自然用語規則（短句、你、術語 backend／store／reload／plugin／daemon 維持英文）。驗證：agent 逐點對照英文版，兩版的點數、指令、設定名稱與連結一樣多。

## 2. 驗證

- [x] 2.1 [after: 1.1] 在 Docker 驗證「檢查整份暫存設定目錄」的做法：建立一個暫存主檔與 `conf.d/` 目錄（含一個有效片段），用 `tools/docker-test.sh` 在容器內以 `emfor --config <暫存主檔> check-config` 跑一次，結果是 `config ok`；再放進一個有錯的片段，結果是結束代碼 2 並指出該檔案。驗證：兩次結果記在回報裡；容器內以 root 跑，所以沒有驗證「以 daemon 的使用者跑」這一部分，小節的文字不可宣稱測過那一部分。
- [x] 2.2 [after: 1.2, 2.1] 執行 `tools/check-manual-links.py`（錨點與連結都通過），再執行 `spectra validate emfor-config-tools-section`。驗證：兩者都通過。

## 3. 補做：對三個工具實測（2.1 之後加入）

- [x] 3.1 在 Debian 12（arm64）的 systemd 容器（`docker run --privileged --cgroupns=host`，映像由 Dockerfile 建出，裝 0.7.0 的 deb、Ansible core 2.14.18、Puppet 7.23.0，Salt 3008.5 用 pip 裝，因為 Debian 12 沒有 Salt 套件）裡，對 `conf.d/10-web.toml` 分別用 Ansible（`ansible.builtin.copy` 的 `validate`，handler 用 `ansible.builtin.service` 的 `state: reloaded`）、Puppet（`file` 的 `validate_cmd`，`service` 的 `restart => '/bin/systemctl reload emfor'`）、Salt（`file.managed` 的 `check_cmd`，`service.running` 的 `reload: True` 加 `watch`）套用三個案例，驗證選項都指向一支 wrapper script（建暫存設定、`umask 022`、`chown` 給 `emfor`、以 `runuser -u emfor` 跑 `emfor --config ... check-config`）。結果：有效片段三個工具都安裝並 reload（journal 出現 `config reloaded`）；有錯的片段（`timeout` 大於 `interval`）三個工具都被驗證擋下、舊檔 `cmp` 相同、沒有 reload；再套用一次有效片段，三個工具都沒有變更。另外：改 `[dns] listen` 後 `systemctl reload emfor` 結束代碼 0，Ansible `state: reloaded` 回報 `changed`，log 出現 `reload rejected`，port 5300 保持、5399 沒開。第一次跑時 wrapper 沒設 `umask`，Ansible 執行它時暫存的 `conf.d` 變成 `0777`，`check-config` 拒絕，加上 `umask 022` 後通過。這一項取代 2.1 的限制（「沒測過以 daemon 的使用者跑」）：現在以 daemon 的使用者跑過。驗證：兩個語言版本的小節最後一段和第 1、3 點寫出的結果，與這裡一致；`tools/check-manual-links.py` 與 `spectra validate` 通過。沒測：Chef、其他版本與系統、從伺服器執行的工具（Puppet server、Salt master）。

## 4. 補做：把 wrapper 放進 examples（使用者要求）

- [x] 4.1 新增 `examples/deploy/emfor-check-staged.sh`（可執行；`sh -n` 通過）：`emfor-check-staged.sh NAME CANDIDATE` 建暫存設定（主檔加 `conf.d`，裡面是真實目錄的所有檔案，但 `NAME` 換成 `CANDIDATE`），自己設 `umask 022`，以 `EMFOR_USER`（預設 `emfor`）身分跑 `emfor --config <暫存主檔> check-config`，結束代碼就是 `check-config` 的（0 或 2）；`EMFOR_CONFIG`、`EMFOR_INCLUDE_DIR` 可指定主檔與 `include_dir` 名稱；檔名含 `/` 或參數數目不對時結束代碼 1。在 Debian 12 的 systemd 容器裡驗證：直接呼叫有效片段得 `config ok`、結束代碼 0，有錯的片段結束代碼 2，呼叫端 `umask 0` 仍通過，以 `emfor` 使用者身分呼叫也通過，暫存目錄用完即刪；再用這支 script 當驗證指令，Ansible、Puppet、Salt 各跑「有效、有錯、再套用一次」三個案例，結果與 3.1 相同（有效：安裝並 reload 一次；有錯：舊檔不變、沒有 reload；再套用：沒有變更、沒有 reload）。沒有把它加進 `tests/deploy` 或 CI。驗證：上述容器測試的結果；兩個語言版本的小節連到它。
