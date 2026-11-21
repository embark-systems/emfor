## Context

`StoreConfig::default` (`crates/emfor-daemon/src/config.rs`) 的 `file_mode` 是 `0o644`、`lock_file_mode` 是 `0o666`。`Store::open`（`crates/emfor-daemon/src/store.rs`）在開啟 LMDB 環境之後、每次啟動都對資料檔與 lock 檔呼叫 `set_permissions`，所以模式不只套在新建的檔案上，也會改現有檔案。`[store]` 在 reload 時要求重啟（`cli.rs` 的 reload 路徑會回報「store settings require restart」），所以這兩個值只在啟動時生效。client 以 `MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS` 開 store（`clients/rust/src/store.rs`），要能讀資料檔，也要能寫 lock 檔來登記 reader。

deb 與 rpm 套件裝的 `/etc/emfor/emfor.toml` 只有 `include_dir = "conf.d"`，沒有 `[store]` 鍵，所以套件使用者用的是程式內的預設值。packaged unit 是 `User=emfor`、`Group=emfor`、`StateDirectoryMode=0755`、`UMask=0027`。

## Goals / Non-Goals

**Goals:**

- 預設設定下，只有 daemon 的使用者與群組能讀 store、寫 lock 檔。
- 管理者仍然能用明確的 `file_mode`／`lock_file_mode` 設回舊值，並在這樣做的時候得到一行警告，知道代價。
- 破壞性的部分（升級後第一次啟動改現有檔案的模式）寫進發佈說明與手冊。

**Non-Goals:**

- 不改套件的設定檔、systemd unit、`StateDirectoryMode`、lock 檔缺少時 client 自動建立的行為。
- 不加遷移工具，不自動管理群組。

## Decisions

### 預設值改在程式，不改套件設定檔

`StoreConfig::default` 直接改成 `0o640`／`0o660`。替代做法是只改套件設定檔與範例：deb 的 conffile 在使用者沒改過時，升級會被靜默換成新版，所以「改套件設定檔」對沒改過設定的套件使用者與改程式預設值有一樣的效果，卻讓 FreeBSD 與手寫設定維持寬鬆，同一個預設值在不同安裝方式下不一樣。改在程式，行為一致，也不需要動套件。

### 警告沿用 credential_warnings 的做法，只在 check-config 與啟動

新增一個與 `credential_warnings`（`crates/emfor-daemon/src/cli.rs`）同型的函式，回傳字串清單；`emfor check-config` 把它寫到標準錯誤，daemon 在啟動時以 `warn` 日誌寫出。不放在 reload：`[store]` 變更本來就要求重啟。不是錯誤，不改 `check-config` 的結束碼，與既有的憑證警告一致。

### 警告看 file_mode 與 lock_file_mode 的「其他人可寫」位元

條件是 `mode & 0o002`，兩個選項各一行。理由：兩個檔案都被 client 對應進記憶體或用來登記讀取；lock 檔可寫的後果是停住 daemon 的 commit，資料檔可寫的後果是任何使用者能改掉 client 讀到的內容，兩者都是「本機任何使用者能影響所有查詢」。「其他人可讀」（例如 0644）不警告：手冊把它當作「本機使用者都可信任」的主機的可接受選擇。
替代做法：只警告 `lock_file_mode`。資料檔與 lock 檔是同一個問題的兩面，只警告其中一個會留下更糟的漏網。

### 警告寫出後果與改法，讓 check-config 的讀者知道怎麼處理

只寫「mode 0666 不安全」不夠：執行 `emfor check-config` 的人通常正要改設定，需要在同一行讀到後果、要改成什麼、還要做什麼、去哪裡看。每一行因此固定四件事：選項與模式、這個模式讓本機任何使用者能做什麼、該選項建議的值（預設值）加上「應用程式的使用者要在 daemon 的群組裡」、手冊的「Who can use the store」一節；還加一句這個值在下次啟動時才套用到現有檔案。草稿（英文，實際措辭由實作決定，spec 只釘住這四件事與預設值）：

- `[store] lock_file_mode = "0666" lets every user on this host write the lock file, which holds LMDB's reader table and the lock the daemon takes for every commit: any local user can stop the daemon's commits, and every client then sees a stale store. Use "0660" (the default) and put the users of your applications in the group of the emfor daemon. The daemon sets this mode on the existing files at every start. See "Who can use the store" in the operations chapter of the manual.`
- `[store] file_mode = "0666" lets every user on this host write the data file that the emfor clients read: any local user can change what every lookup returns. Use "0640" (the default) and put the users of your applications in the group of the emfor daemon. ...`（後半同上）

替代做法：只指向手冊、不在行裡寫改法。check-config 的輸出常被貼進工單或 CI 日誌，讀的人未必有手冊；改法放在行裡，一行就能行動。

## Implementation Contract

- **行為**：沒寫 `[store] file_mode` 與 `lock_file_mode` 的 daemon，啟動後資料檔是 `0640`、lock 檔是 `0660`（包含啟動前就存在、模式不同的檔案）。明確寫的值照舊。
- **警告**：`file_mode` 或 `lock_file_mode` 有 `0002` 位元時，`emfor check-config` 在標準錯誤、daemon 在啟動時以 `warn` 各寫一行，每個有問題的選項一行，每行寫出：選項與八進位模式、這個模式讓本機任何使用者能做什麼、建議的值（`lock_file_mode` 是 `"0660"`、`file_mode` 是 `"0640"`）與應用程式的使用者要在 daemon 的群組裡、手冊「Who can use the store」一節、以及這個值在下次啟動才套用到現有檔案；結束碼與啟動結果不變。預設值不產生警告。
- **設定解析**：`file_mode` 與 `lock_file_mode` 的格式驗證（八進位字串、`"999"` 是錯誤）不變。
- **驗收**：`store.rs` 的 `creates_files_with_configured_modes` 斷言 0640／0660；新單元測試涵蓋警告的所有組合；新整合測試以 `emfor check-config` 與實際啟動確認警告出現且 daemon 照常運作；`tools/docker-test.sh` 跑 fmt、clippy 與整個 workspace 的測試。
- **範圍內**：`config.rs` 預設值、`cli.rs` 警告、相關測試、`examples/emfor.toml`、手冊（英文與繁體中文）、`docs/deploy/daemon.md`、CHANGELOG。
- **範圍外**：套件設定檔與 unit、client、`[store]` 的 reload 行為、TODO 58 ⑥。

## Risks / Trade-offs

- [升級後第一次啟動，不在 daemon 群組的應用程式失敗（`UNAVAILABLE`）] → CHANGELOG 與手冊寫明升級前先把應用程式的使用者加進群組，或設回 0644／0666；這是使用者已接受的破壞性變更。
- [群組名稱因安裝方式而異] → 文件寫「daemon 的群組」，並列出 deb 與 rpm 的 `emfor`；FreeBSD 的群組沒有在這個 change 查證，文件只說「daemon 使用者的群組」。
- [有人以 0666 繼續運作，只多一行警告] → 警告是刻意不阻擋：既有的寬鬆安裝在明確選擇下仍然可用。

## Migration Plan

給從 0.6 升級的主機，在 0.7.0 發佈後：

1. 升級之前，決定要走哪一條：把每個應用程式的使用者加進 daemon 的群組（deb 與 rpm 是 `emfor`）；或在 `[store]` 明確寫 `file_mode = "0644"` 與 `lock_file_mode = "0666"`（會得到啟動警告）。
2. 升級並重啟 daemon。daemon 啟動時把現有的資料檔與 lock 檔改成新模式（檔案的群組不變）。
3. 確認應用程式能查詢（`emfor lookup` 以應用程式的使用者執行）。

回復：在 `[store]` 設回 `"0644"` 與 `"0666"` 並重啟 daemon，即回到 0.6 的行為。
