## Context

daemon 的子程序有兩種：`exec` 檢查的指令（`ExecChecker::check`，每次檢查啟動一次）與外掛（`PluginHost` 的 `launch`，長駐，crash 後重啟）。兩處都用 `tokio::process::Command`，設了 `current_dir("/")`、自己的 process group、stdio，再呼叫 `crate::child::inherit_stdio_only`；**都沒有碰環境**，所以子程序得到 daemon 的整份環境，`exec` 另外多三個 `EMFOR_*`。

已查證的現況：
- `exec` 的 `check_options` 只接受 `command`（`ExecFactory::validate` 用 `reject_unknown_keys(options, &["command"], ..)`）；指令驗證在共用的 `parse_command`（`checker/exec.rs`），外掛的 `command` 也用它。
- 外掛宣告由 `config.rs` 的 `plugins()` 解析成 `PluginSpec { name, command }`，只接受 `command` 一個鍵（`unknown_keys(t, &path, &["command"])`）。
- 外掛 reload 時 `PluginSet::prepare`（`plugin/set.rs`）用 `h.command() == spec.command` 判斷能否沿用執行中的外掛；`plugin_checker_id`（`plugin/mod.rs`）以名稱與 command 組成檢查器 id。
- 外掛測試用 `/usr/bin/env VAR=1 <test plugin>` 當 `command`（`tests/plugin_host.rs` 的 `command()`、`tests/plugins.rs` 的 `plugin_decl`）：變數是 `env` 這支程式在子程序裡設的，不是從 daemon 繼承，清環境不影響它們。
- 隨套件出貨的 `emfor.service`（`packages/common/`、`examples/systemd/`）沒有 `Environment=`／`PATH`，daemon 的環境就是 systemd 給的預設環境。**未查證**：該預設 `PATH` 的確切值（依 systemd 建置而不同）與 FreeBSD rc 腳本給 daemon 的環境；這兩處不在本變更的範圍內，只影響「`PATH` 白名單傳下去的值是什麼」。

## Goals / Non-Goals

**Goals:**

- exec 指令與外掛預設只拿到白名單環境，daemon 環境裡的其他變數（尤其是祕密）不會傳下去。
- 需要其他變數的安裝，能用一個明確的設定（名稱清單）加回來，且不需要把值寫進設定檔。
- 行為由測試證明：沒列入的變數看不到、列入的看得到、名稱驗證錯誤指到正確的設定鍵。

**Non-Goals:**

- 不改 store、peers、DNS、clients、`examples/` 的腳本、RADIUS／pgsql 的檔案權限。
- 不提供在設定檔裡寫變數「值」的方式。
- 不改 unit 檔與 rc 腳本。
- 不限制子程序用 `PATH` 找到什麼程式（`command` 第一個元素本來就必須是絕對路徑，白名單不改這個）。

## Decisions

### 白名單與 `env_pass` 共用一個函式

`crates/emfor-daemon/src/child.rs` 新增 `pub(crate) fn set_environment(cmd: &mut Command, pass: &[String])`，它只是以 `std::env::vars_os()` 呼叫 `set_environment_from(cmd, vars, pass)`；後者先 `env_clear()`，再從傳入的 `(OsString, OsString)` 序列把名稱為 `PATH`、`LANG`、`TZ`、以 `LC_` 開頭（`LC_ALL`、`LC_CTYPE` 等）的變數，以及名稱在 `pass` 裡的變數設給子程序。值用 `OsString` 原樣傳（非 UTF-8 也照傳）。`pass` 裡的名稱在 daemon 環境沒有時就不設，不是錯誤（設定檔在不同主機上共用，環境不同是常態）。`exec` 之後再用 `.env("EMFOR_IP", ..)` 等設三個變數，所以它們一定蓋過任何來源。

`set_environment_from` 存在的理由：測試可以直接餵一份假的「daemon 環境」，不必改本程序的全域環境（Rust 2024 的 `std::env::set_var` 是 `unsafe`，也會干擾平行的其他測試）。

理由：兩處子程序用同一套規則，只在 `child.rs` 一處維護；`inherit_stdio_only` 已經在同一個模組，是同類的「子程序啟動前整理」。

### 名稱驗證：`parse_env_pass`

`child.rs` 另有 `pub(crate) fn parse_env_pass(value: Option<&toml::Value>) -> Result<Vec<String>, String>`：缺少時是空清單；必須是字串陣列（`must be an array of strings`）；每個名稱必須符合 `[A-Za-z_][A-Za-z0-9_]*`（`invalid variable name "<name>"`，這也排除了 `=` 與 NUL）；不可重複（`duplicate variable name "<name>"`）。`exec` 另外拒絕 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE` 這三個名稱（`<name> is set by emfor`），因為它們由 emfor 設定，不能被 daemon 的環境蓋掉；其他以 `EMFOR_` 開頭的名稱（例如使用者自己的 `EMFOR_PREFIX`）合法。這是實作時改過的決定：原本寫成拒絕整個 `EMFOR_` 前綴，但既有的 `tests/peers.rs` 需要傳 `EMFOR_TEST_FLAG`，顯示這太寬，使用者決定只擋那三個。外掛不受這條限制（emfor 不給外掛 `EMFOR_*`）。空陣列合法，等於沒設。

### 設定位置

- `exec`：`check_options.env_pass`。`ExecFactory::validate` 接受的鍵由 `["command"]` 改為 `["command", "env_pass"]`，`ExecChecker` 多一個 `env_pass: Vec<String>` 欄位。錯誤指到 `services."<service>".check_options.env_pass`。`env_pass` 是服務設定的一部分，改變它走服務設定變更的既有路徑，不需要特別處理。
- 外掛：`[plugins."<name>"] env_pass`。`config.rs` 的 `unknown_keys` 允許清單加 `env_pass`，`PluginSpec` 多 `env_pass: Vec<String>`。錯誤指到 `plugins.<name>.env_pass`。

### 外掛：`env_pass` 是外掛身分的一部分

`PluginHost::start(name, command)` 保留，改為呼叫新的 `PluginHost::start_with(name, command, env_pass)`（24 個既有呼叫點不用改）；`PluginSet::prepare` 用 `start_with`，並以「`command` 與 `env_pass` 都相同」判斷能否沿用；`PluginHost` 多 `env_pass()` 取值。`launch` 與每次 crash 後的重啟都用 host 存的 `env_pass`。`plugin_checker_id` 在 `env_pass` 非空時把清單加進 id（空時格式不變，id 不變），所以 `env_pass` 改變會得到新的檢查器 id，與 `command` 改變同樣處理：reload 時重啟該外掛，用到它的服務的後端從 `unknown` 重新開始。

### 不做的備案

- `env_clear = true` 開關、預設仍繼承：預設不安全，是否有收益取決於每位操作者是否知道，否決。
- 設定檔裡寫 `env = { A = "b" }`：把祕密值寫進設定檔，多一個存放處，且與設定檔的權限模型衝突，否決；要傳值就在 unit 檔用 `Environment=`／`EnvironmentFile=` 設給 daemon，再用 `env_pass` 列名稱。
- 白名單多放 `HOME`、`USER`：預設不放，因為這正是祕密與設定常藏的地方；需要的人用 `env_pass`。

## Implementation Contract

**行為（操作者看到什麼）**

- `exec` 檢查的指令與外掛的環境，只有：`PATH`、`LANG`、`TZ`、`LC_*`（取自 daemon，沒有就沒有）、`env_pass` 列出且 daemon 有的變數；`exec` 另有 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`。其他變數（例如 daemon 環境裡的 `SECRET_TOKEN`、`HOME`、`http_proxy`）看不到。
- `check_options.env_pass = ["HOME"]` 的 `exec` 檢查看得到 `HOME` 的值（daemon 有設時）。`[plugins."x"] env_pass = ["http_proxy"]` 同理。
- 無效的 `env_pass` 是設定錯誤，`emfor check-config` 以 2 結束並指出鍵：`services."<s>".check_options.env_pass` 或 `plugins.<name>.env_pass`。

**介面／資料形狀**

- TOML：`env_pass = ["NAME", ...]`（字串陣列，只放名稱）。
- 函式：`child::set_environment(&mut Command, &[String])`、`child::parse_env_pass(Option<&toml::Value>) -> Result<Vec<String>, String>`、`PluginHost::start_with(&str, &[String], &[String])`、`PluginHost::env_pass() -> &[String]`、`PluginSpec.env_pass: Vec<String>`、`ExecChecker.env_pass: Vec<String>`。

**失敗模式**

- `env_pass` 的名稱在 daemon 環境沒有：不傳，不報錯、不記 log。
- 名稱無效或重複、`exec` 的 `EMFOR_*` 名稱、型別錯誤：設定錯誤（見上）。
- 子程序因缺少某變數而失敗：由該檢查／外掛本身的結果表現（`exec` 是 `down`／`error`、外掛是啟動或握手失敗的設定錯誤）；emfor 不猜哪個變數少了。

**驗收**

- 單元測試（`child.rs`）：`set_environment_from` 在一份設了 `SECRET_TOKEN`、`PATH`、`LC_ALL`、`TZ`、`LANG`、`HOME` 的 daemon 環境下，子程序（`/usr/bin/env`）的輸出只含白名單與 `pass` 列出的名稱；`pass` 中 daemon 沒有的名稱不出現；非 UTF-8 值原樣傳。測試不改本程序的環境（見上）。
- `exec` 測試（`checker/exec.rs`）：指令印出環境時看不到未列入的變數、看得到 `env_pass` 列入的變數與 `EMFOR_*`；`env_pass` 含 `EMFOR_IP` 的設定被拒；錯誤鍵正確。
- 外掛測試（`tests/plugin_host.rs`、`tests/plugins.rs`）：外掛的環境符合上述；`env_pass` 改變使 reload 重啟外掛（`command` 不變）；無效 `env_pass` 的設定錯誤鍵。
- `tests/nagios`（Docker）與完整 gate（`tools/docker-test.sh`）通過。
- 手冊連結檢查（`python3 tools/check-manual-links.py`）通過。

**範圍**

- 在範圍內：`child.rs`、`checker/exec.rs`、`plugin/host.rs`、`plugin/set.rs`、`plugin/mod.rs`、`config.rs` 的 `PluginSpec`，上述測試，手冊與 CHANGELOG。
- 不在範圍內：見 Non-Goals。

## Risks / Trade-offs

- **相容性（行為破壞性）**：依賴 `HOME`、proxy 變數、`LD_LIBRARY_PATH`、`JAVA_HOME` 等的現有檢查與外掛，升級後缺少這些變數。緩解：CHANGELOG 與手冊明說，並給 `env_pass` 的寫法；錯誤只會出現在那個檢查（`down`／`error`）或外掛啟動失敗，不影響其他服務。
- `PATH` 的值取決於 daemon 怎麼啟動：本變更不改它，只照傳。若 daemon 的 `PATH` 很短，指令裡不用絕對路徑呼叫其他程式會找不到，與現在相同。
- 預設 `LANG`／`LC_*`／`TZ` 傳下去可能改變指令輸出的語言與時區，與現狀相同（現在也繼承）。
- 本機測試用的 `/usr/bin/env VAR=1` 寫法不受影響；只有依賴「daemon 自己的環境被繼承」的測試才會需要 `env_pass`。已查：`tests/` 裡的外掛測試都用 `env` 包裝；`exec` 測試與 `tests/nagios` 是否依賴繼承的變數，在實作的第一個任務查證（見 tasks）。

## Migration Plan

沒有資料遷移。升級者若有依賴環境變數的檢查或外掛：在該 `exec` 的 `check_options` 或 `[plugins."<name>"]` 加 `env_pass = ["NAME"]`，`emfor check-config` 驗證，`SIGHUP` 套用（外掛因 `env_pass` 改變而重啟，exec 檢查在下一次檢查生效）。回退：換回上一版 binary，設定檔裡的 `env_pass` 會被舊版拒絕為未知的鍵，要先拿掉。

## Open Questions

None
