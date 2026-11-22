## Why

第 5 章「Generating the config」只說「用設定管理工具（Ansible、Puppet、Salt）或腳本產生設定」，列了四個做法，沒有說這些工具的預設做法在 emfor 上會出什麼問題。照預設做法寫的人會踩到這幾件事：

- handler 預設 restart。restart 會有一小段時間，lookup 分不出好壞 backend（第 12 章）。
- 工具內建的驗證選項（Ansible 的 `validate`、Puppet 的 `validate_cmd`、Salt 的 `check_cmd`）只會把一個暫存檔交給指令。`emfor check-config` 讀的是主設定檔和它的 `include_dir`，沒有「只檢查一個片段」的參數，所以這些選項不能直接用。
- 0.7.0 起，`check-config` 會啟動設定裡宣告的 plugin；用 root 跑，plugin 就以 root 執行。
- 設定檔和 peer secret 有擁有者與權限的規則，工具的暫存檔也要符合。
- deb、rpm 沒有發佈在任何套件庫，工具沒有地方可以裝。

這些都已經散在第 4、5、11、12、13 章，但沒有一處寫給「用設定管理工具」的人看。

## What Changes

在 `docs/manual/05-configuring.md` 的「Generating the config」之後加一小節，並在 `docs/manual.zh_TW/05-configuring.md` 加對應的繁體中文小節。內容是工具中立的檢查清單，五點：

1. 用 reload，不要 restart。Ansible 的 `service` 模組有 `state: reloaded`；Salt 的 `service.running` 有 `reload: True`（要搭配 `watch`）；Puppet 的 `service` 用 `restart` 屬性指定 reload 指令。
2. 驗證要用 daemon 的使用者跑 `emfor check-config`（0.7.0 起它會啟動 plugin）。
3. 工具內建的驗證選項（Ansible `validate`、Puppet `validate_cmd`、Salt `check_cmd`）不能直接用，因為它們只傳一個暫存檔，而 `check-config` 要整份設定。可行的做法：用 `emfor --config <暫存主檔> check-config` 檢查一整份暫存設定目錄（`include_dir` 相對於主設定檔所在的目錄），或像 `examples/deploy/emfor-drain.sh` 那樣先寫、檢查、失敗就放回舊檔。
4. 設定檔和 peer secret 的擁有者與權限：root 或 daemon 的使用者擁有，群組與其他人不可寫；peer secret 另外不可被其他人讀。暫存檔也要符合。
5. 套件從哪來：deb、rpm 沒有發佈在任何套件庫，要自己建（第 4 章）。

每一條最多附一行三個工具各自對應的設定名稱，並在小節最後寫出實際測過的環境、版本與沒測過的部分（見 tasks.md 3.1）。下列名稱都已對照官方文件或原始碼查證：Ansible `ansible.builtin.template` 的 `validate`（用 `%s` 代表暫存檔）和 `ansible.builtin.service` 的 `state: reloaded`；Puppet `file` 的 `validate_cmd`（用 `%` 代表輸入檔，失敗時資源失敗）和 `service` 的 `restart`；Salt `file.managed` 的 `check_cmd`（暫存檔附在指令最後，退出碼非零就不改檔）和 `service.running` 的 `reload`。沒查到的不寫。

另外（使用者後來要求）加一支 `examples/deploy/emfor-check-staged.sh`：第 3 點提到的 wrapper，建暫存設定、自己設 `umask`、以 daemon 的使用者跑 `emfor --config ... check-config`，可以直接當 Ansible `validate`、Puppet `validate_cmd`、Salt `check_cmd` 的指令；沿用 `emfor-drain.sh` 的環境變數（`EMFOR_USER`、`EMFOR_CONFIG`），手冊的小節連到它。

## Non-Goals

- 不寫 Puppet 或 Salt 的完整範例，不把三個工具的測試設定與映像放進 repo。（本來的理由是這裡沒有人能跑這些工具，專案不宣稱沒測過的東西；後來使用者要求在 Docker 驗證，所以改成「實際跑過再寫」：Ansible、Puppet、Salt 都在 Debian 12 的 systemd 容器裡對 0.7.0 的 deb 跑過，結果見 tasks.md 的 3.1。三個工具的測試設定留在 session 的 scratchpad，沒有進 repo；只有驗證用的 wrapper 經使用者要求放進 `examples/deploy/`，見 What Changes。）
- 不改程式行為，不改 spec，不改第 5 章以外的章節（其他章節只被連結）。
- 不處理 Chef、Terraform 等其他工具。
- 不改 `docs/TODO.md`。

## Impact

- Affected specs: none
- Affected code:
  - New: examples/deploy/emfor-check-staged.sh
  - Modified: docs/manual/05-configuring.md、docs/manual.zh_TW/05-configuring.md
  - Removed: (none)
- Compatibility: no capability-level observable behavior changes
