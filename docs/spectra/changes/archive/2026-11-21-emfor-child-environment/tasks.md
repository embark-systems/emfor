## 1. 共用的環境設定

- [x] 1.1 （設計「不做的備案」：不加 `env_clear` 開關、不在設定檔寫值，所以只有名稱清單）在 `crates/emfor-daemon/src/child.rs` 新增 `set_environment_from(cmd, vars, pass)`（先 `env_clear`，再傳 `PATH`、`LANG`、`TZ`、`LC_*` 與 `pass` 列出的變數，值以 `OsString` 原樣傳）、以 `std::env::vars_os()` 呼叫它的 `set_environment`，以及 `parse_env_pass`（驗證規則見 design「名稱驗證」）；完成時：單元測試在一份假的 daemon 環境（含 `SECRET_TOKEN`、`HOME`、`PATH`、`LANG`、`LC_ALL`、`TZ`、一個非 UTF-8 值）下證明子程序（`/usr/bin/env`）只有白名單與 `pass` 的變數、`pass` 中不存在的名稱不出現、非 UTF-8 值原樣傳；`parse_env_pass` 的測試涵蓋 design 與 spec 例表的每一列；驗證：在 Docker 跑這些測試（`tools/docker-test.sh`），並做一次破壞檢查（拿掉 `env_clear`，確認「看不到 `SECRET_TOKEN`」的測試失敗）。

## 2. exec 檢查

- [x] 2.1 [after: 1.1] （需求「Exec checker」、設計「設定位置」）`ExecFactory::validate` 接受 `env_pass`（`reject_unknown_keys` 允許 `command`、`env_pass`；`EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE` 三個名稱的錯誤為 `<name> is set by emfor`，其他 `EMFOR_*` 名稱合法，錯誤鍵為 `env_pass`），`ExecChecker::check` 在設三個 `EMFOR_*` 之前呼叫 `set_environment`；先確認 `checker/exec.rs` 內既有測試（用 `script()` 寫的 `/bin/sh` 腳本）與 `tests/nagios`、`examples/nagios/emfor-exec.sh` 沒有依賴被繼承的 daemon 變數（讀程式並跑一次）；完成時：新測試證明指令的環境看不到未列入的變數、看得到 `env_pass` 與 `EMFOR_*`、`env_pass = ["EMFOR_IP"]` 被拒、其他無效輸入的錯誤訊息與 spec 例表一致；驗證：Docker 內 `tools/docker-test.sh` 全過，並對 `set_environment` 呼叫做破壞檢查。

## 3. 外掛

- [x] 3.1 [after: 1.1] （需求「Plugin registration in config」、設計「設定位置」）`config.rs` 的 `PluginSpec` 多 `env_pass: Vec<String>`，`[plugins."<name>"]` 接受 `env_pass`（`unknown_keys` 允許清單加入，驗證用 `parse_env_pass`，錯誤鍵 `plugins.<name>.env_pass`）；完成時：設定驗證測試涵蓋 spec 例表新增的四列（`["http_proxy"]` 合法、`["A B"]`、重複名稱、非陣列）；驗證：Docker 內測試通過，`emfor check-config` 對一份含無效 `env_pass` 的設定以 2 結束並指出 `plugins.<name>.env_pass`。
- [x] 3.2 [after: 3.1] （需求「Plugin environment」與「Plugins on reload」、設計「外掛：`env_pass` 是外掛身分的一部分」）`PluginHost::start_with(name, command, env_pass)`（`start` 改為以空清單呼叫它，24 個既有呼叫點不改）、`PluginHost::env_pass()`、`launch` 與 crash 後的重啟都呼叫 `set_environment`、`PluginSet::prepare` 以「`command` 與 `env_pass` 都相同」決定沿用、`plugin_checker_id` 在 `env_pass` 非空時把清單加進 id（空時格式不變）；完成時：測試用 `/bin/sh -c 'env >&2; exec <test plugin>'` 形式的 `command`（與 `tests/plugin_host.rs` 既有的 `command_with_child` 同型）證明外掛看不到未列入的變數、看得到 `PATH` 與 `env_pass` 列入的變數、重啟後仍然如此；`tests/plugins.rs` 的 reload 測試證明只改 `env_pass` 會重啟外掛、`command` 與 `env_pass` 都不變時沿用同一個 pid、空 `env_pass` 的 id 與修改前相同；驗證：Docker 內 `tools/docker-test.sh` 全過。

## 4. 文件與驗收

- [x] 4.1 [after: 2.1, 3.2] 先 grep 再更新 `docs/manual/04-health-checks.md`（exec 一節與外掛一節）、`docs/manual/03-configuration.md`（`[plugins]` 一節）、`docs/manual/10-migrating.md`（若提到環境）、`docs/manual/08-operations.md`（只在 Nagios 一節有影響時）與 `docs/manual.zh_TW/` 對應檔：說明預設環境、`env_pass` 與依賴 `HOME`／proxy 變數的升級做法；完成時：英文與 zh_TW 的描述一致，`python3 tools/check-manual-links.py` 無問題；驗證：內容審閱加該腳本。
- [x] 4.2 [after: 4.1] `CHANGELOG.md` Unreleased 加一筆（行為破壞性，說明受影響的安裝與 `env_pass` 的修法，Checked／Not verified：未在 macOS／FreeBSD 驗證，未測真實的 systemd 預設 `PATH`），並跑完整驗收；完成時：Docker 內 `tools/docker-test.sh`（fmt、clippy `-D warnings`、全部測試）通過、`tests/nagios/run.sh` 在 Docker 通過、`spectra analyze` 與 `spectra validate` 乾淨；驗證：以上輸出。
- [x] 4.3 （需求「Check definition digest」）review 前的查證發現 `peer-group` spec 把外掛的檢查器身分寫成 `plugin:<name>:<command as JSON>`，與 3.2 讓 `env_pass` 進入 id 的實作不符：以本 change 的 `specs/peer-group/spec.md` 補上這條需求（`env_pass` 進入摘要、空時格式不變）；完成時：delta spec 通過 `spectra validate`，3.2 的 `checker_id_has_env_pass_only_when_there_is_one` 測試即是它的程式驗證；驗證：內容審閱與 `spectra analyze`。
