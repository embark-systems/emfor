## Why

daemon 啟動的子程序（`exec` 檢查的指令、外掛）現在繼承 daemon 的整份環境變數：`ExecChecker::check`（`crates/emfor-daemon/src/checker/exec.rs`）與外掛的 `launch`（`crates/emfor-daemon/src/plugin/host.rs`）都沒有清環境，只做 `inherit_stdio_only`（關掉 stdio 以外的 fd）。daemon 的環境裡若有 unit 檔或 rc 腳本設的敏感值（密碼、token、雲端憑證），每一次 exec 檢查與每個外掛都拿得到，檢查腳本或外掛的任何一個 bug 或記錄行為都可能把它帶出去。這是 Fable review 留下的低嚴重度項目（TODO 的 `env_clear`），之前因為會讓依賴某個環境變數的現有安裝壞掉，需要相容策略才擱置；策略已與使用者定案：預設白名單，加一個讓安裝自己加回名稱的設定。

## What Changes

- `exec` 檢查的指令與外掛的子程序改成先清空環境，只傳白名單：`PATH`、`LANG`、`TZ`、名稱以 `LC_` 開頭的變數，取自 daemon 自己的環境；`exec` 另外照舊傳 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`。外掛不傳 `EMFOR_*`。
- 新設定 `env_pass`：一串變數**名稱**（值取自 daemon 的環境，不在設定檔裡寫值），把白名單以外的變數也傳給該子程序。`exec` 放在 `check_options.env_pass`，外掛放在 `[plugins."<name>"] env_pass`。名稱必須符合 `[A-Za-z_][A-Za-z0-9_]*`，不可重複；`exec` 的 `env_pass` 不可包含 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`（這三個由 emfor 設定，不能被覆蓋）。daemon 環境裡沒有的名稱就不傳，不是錯誤。
- 外掛的 `env_pass` 改變時，與 `command` 改變同樣處理：reload 時重啟該外掛。
- 這是**行為破壞性**變更：依賴 `HOME`、proxy 變數（`http_proxy` 等）、`LD_LIBRARY_PATH` 或其他 daemon 環境變數的現有檢查與外掛，升級後會少了這些變數，要在 `env_pass` 列出。CHANGELOG、手冊（中英）要明說，並給出怎麼修。
- 手冊 04 章 exec 與外掛段落、必要時 08 章（Nagios 一節）與 zh_TW 對應檔更新；CHANGELOG Unreleased 記一筆。

## Non-Goals

- 不改 store、peers、DNS、clients，也不改 `examples/` 的 `drill`／`pfctl` 腳本（它們由使用者的 shell 執行，不是 daemon 的子程序）。
- 不處理 RADIUS／pgsql 的 `password_file`、`tls_key_file` 權限檢查（另案，已決定不修）。
- 不在設定檔裡直接寫變數的值（`env = { A = "b" }`）：值寫在設定檔會多一個存放秘密的地方，需要時由 unit 檔的 `Environment=` 設給 daemon，再用 `env_pass` 傳。
- 不改 daemon 本身怎麼取得環境（unit 檔、rc 腳本不動）。
- 被否決的做法：只加 `env_clear = true` 開關而預設不變（預設不安全，只有知道的人才有收益）；預設白名單而不可加名稱（依賴其他變數的外掛會沒有出路）。

## Capabilities

### New Capabilities

（無新的 capability；`Plugin environment` 是 `checker-plugins` 的新需求）

### Modified Capabilities

- `health-checking`: `Exec checker` 需求：子程序環境改為白名單，加 `check_options.env_pass`，接受的鍵由只有 `command` 改為 `command` 與 `env_pass`。
- `checker-plugins`: `Plugin registration in config` 需求加 `env_pass`；`Plugins on reload` 需求把 `env_pass` 改變視同 `command` 改變（重啟外掛）；新增 `Plugin environment` 需求描述外掛的環境。
- `peer-group`: `Check definition digest` 需求：外掛的 `env_pass` 非空時進入檢查器身分（因此進入摘要）；`exec` 的 `env_pass` 在 `check_options` 裡，本來就在摘要裡。各主機的 `env_pass` 不同，就是不同的檢查定義，與 `command` 不同一樣。

## Impact

- 程式：`crates/emfor-daemon/src/child.rs`（新增共用的環境設定與 `env_pass` 驗證）、`crates/emfor-daemon/src/checker/exec.rs`、`crates/emfor-daemon/src/plugin/host.rs`、`crates/emfor-daemon/src/plugin/set.rs`、`crates/emfor-daemon/src/plugin/mod.rs`、`crates/emfor-daemon/src/config.rs`（`PluginSpec`）。
- 測試：`crates/emfor-daemon/tests/` 的外掛與 exec 測試；`tests/nagios`（Docker）。
- 文件：`docs/manual/04-health-checks.md`、外掛說明所在章節、`docs/manual/08-operations.md`（只在有影響時）與 `docs/manual.zh_TW/` 對應檔、`CHANGELOG.md`。
- 相容性：見上，行為破壞性變更。
