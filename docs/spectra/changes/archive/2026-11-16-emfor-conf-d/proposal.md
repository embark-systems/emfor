## Why

emfor 讀的設定只能是一個檔案。服務多、或由不同團隊或不同工具各管一部分時，管理者得自己把片段合併成單一檔案再安裝；工具（Ansible、Puppet、套件的 postinst）也無法只改自己負責的那一塊。其他守護程式的慣例是 `conf.d`：一個目錄裡的多個檔案，依檔名順序讀，後面的蓋掉前面的。

手冊第 10 章（從 HAProxy 或 Consul 搬過來）也把「多個設定檔」列為目前沒有對應的功能。

## What Changes

- 主設定檔新增頂層選用鍵 `include_dir = "<path>"`：一個目錄，相對路徑相對於主設定檔所在目錄，絕對路徑也可以，只能一個目錄。daemon 讀主設定檔，再依檔名順序讀該目錄裡的所有檔案，在驗證之前合併成一份設定；主設定檔是最底層，後面的檔案蓋掉前面的。
- 讀哪些檔案：一般檔案，以及指向一般檔案的符號連結，名稱以 `.toml` 結尾且不以 `.` 開頭；順序是檔名的位元組字典序（所以要用 `01-`、`10-`、`99-` 補零）；檔名不是 UTF-8 的略過並警告；其他（`foo.toml~`、`.x.toml.swp`、`x.toml.disabled`）都忽略，所以改名就能讓一個檔案不被讀。
- 合併規則：只有「表加表」逐鍵遞迴合併；其他（表與純量、表與清單、純量、清單，包含 `backends` 與 `command`）由後面的值整個取代。後面的檔案不能刪除前面定義的服務、plugin 或 backend。
- 只准在主設定檔的鍵：`[store]` 與 `include_dir` 本身。`emfor status` 與 `emfor lookup` 只讀主設定檔的 `[store]`，所以片段不能改它；片段含有這兩者就是錯誤（也排除遞迴 include）。其他表（plugins、services、defaults、dns、metrics、peers、log 與頂層 `site`）可以出現在片段裡並合併。
- 權限（直接拒絕）：include 目錄，或其中任何被讀的檔案，只要 group 或其他人可寫（`mode & 0o022`），載入就失敗並指出路徑，因為片段可以定義 plugin 的 `command` 或 `exec` 檢查，能寫目錄的人就能以 daemon 的身分執行程式。主設定檔本身不加新檢查。
- `include_dir` 指向的目錄不存在是設定錯誤。啟動時記錄錯誤並以結束碼 2 結束；reload 時目錄消失就拒絕這次 reload、保留舊設定繼續執行、記錄錯誤，daemon 不結束。
- 錯誤帶出檔名：合併時記錄「鍵路徑到最後設定它的檔案」，驗證錯誤歸給最近的祖先鍵所在的檔案；TOML 語法錯誤顯示 `檔名:行:欄`。兩個檔案的值合起來才出錯時，歸屬只是盡力而為。
- `emfor check-config` 在設定了 `include_dir` 時，在原有的 `config ok: ...` 行之後印出讀了哪些檔案（主檔在前，片段依序）；沒設定 `include_dir` 時輸出不變。daemon 在啟動與 reload 時用日誌記下同一份清單。
- 內部：`crates/emfor-daemon/src/cli.rs` 的 `load` 目前把主設定檔當文字重新解析六次（`parse_plugins`、`parse`、`parse_store`、`parse_dns_listen`、`parse_metrics_listen`、`parse_peers_listen`）。改成一個函式讀全部檔案並合併成一份 `toml::Table`，這六個全部改吃它；否則片段裡宣告的 plugin 不會啟動，片段改 `[dns]`、`[metrics]`、`[peers]` 的 listen 也躲得過 reload 的「需要重啟」檢查。以先新增、再遷移、最後收掉的順序進行，並保留 `parse(text, &registry)` 這個文字入口，讓現有的 32 個設定測試不必修改。
- 文件與套件：手冊第 2、3、8、10 章（中英）、`docs/deploy`、`examples/emfor.toml`（`include_dir` 以註解示範）、FreeBSD port（`PLIST` 加目錄、`Makefile` 建立目錄）、CHANGELOG 的 Unreleased。
- 隨 0.5.0 之後的下一版發佈，這次不打 tag。

## Non-Goals

- 不做「印出合併後的有效設定與每個鍵的來源」。
- 不支援多個 include 目錄（清單）。
- 不讓後面的檔案刪除前面定義的東西；要刪就改前面的檔案，或把那個檔案改名使它不被讀。
- 不對主設定檔本身新增權限檢查：現在沒有，現有安裝可能是 `0664`，加了會讓升級後起不來。這是檢查範圍的不對稱，設計裡會說明。
- 不做 `drain = true`（另一個項目）、不改聯防群組的設定、不改 reload 能套用什麼（手冊「What a reload changes」那張表仍然成立，只是對象變成合併後的設定）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: 新增 `include_dir` 與片段目錄的要求（讀哪些檔案、合併規則、只准主檔的鍵、權限、目錄不存在、錯誤帶檔名、check-config 與日誌列出檔案、分檔後的驗證與 reload 與單一檔案一致）。現有的要求（TOML 格式、驗證規則、`check-config`、啟動遇到無效設定、SIGHUP reload）沒有被改寫；新要求在 `include_dir` 存在時補充它們，沒設定 `include_dir` 時行為與現在完全相同。

## Impact

- 程式：`crates/emfor-daemon/src/config.rs`（讀取與合併、檔案篩選、權限檢查、`ConfigError` 帶檔名、`Validator` 查歸屬、新的吃 table 的輔助函式）、`crates/emfor-daemon/src/cli.rs`（`load` 與 check-config、reload 的錯誤處理與日誌）。
- Spec：`docs/spectra/specs/service-config/spec.md`。
- 文件與套件：`docs/manual/` 與 `docs/manual.zh_TW/` 的第 2、3、8、10 章、`docs/deploy/daemon.md`、`examples/emfor.toml`、`examples/freebsd/Makefile`（與 `README.md`）、`CHANGELOG.md`。
- 相容性：新增選用頂層鍵，屬 `docs/compatibility.md`「Config file」允許的相容變更；舊版 daemon 讀到 `include_dir` 會立刻以「unknown key」失敗，可以接受。不影響 store 格式、plugin 協定、client、DNS、metrics。
