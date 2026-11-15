## Why

要知道「我的程式現在會拿到哪一台 backend、為什麼」，今天沒有簡單的辦法：

- `emfor status` 只說 daemon 發佈了什麼狀態，不回答「這個查詢會落在哪一層候選、為什麼這台在或不在候選裡」。
- 要得到答案只能寫一段程式呼叫 client 函式庫，或對 daemon 的 DNS 做 `dig`；沒有 client 函式庫的 shell 腳本與 cron 工作做不到前者，後者看不到原因。
- 值班的人被叫醒時最常問的就是這個問題：backend 明明健康，為什麼流量沒去？或 backend 其實剛失敗，為什麼還被選到？答案藏在候選規則裡（primary 與 backup、通過檢查與失敗中、`unknown`、慢啟動、站台、快組、stale），不看原始碼無法判斷。

## What Changes

- 新增子命令 `emfor lookup <service> [--key <k>] [--explain] [--json] [--dns A|AAAA]`，放在 `emfor` 二進位檔裡，與 `status`、`server` 並列。
  - 預設走 client 的語意：讀 store、判斷 stale、算候選、挑一台，印出 `ip:port`，等於「一個新程序用 client 函式庫會得到的答案」。
  - `--explain` 說明為什麼：store 是否 fresh、候選落在哪一層、每個 backend 為什麼在或不在候選裡、`balance` 怎麼挑、`on_all_down` 怎麼決定。
  - `--dns A|AAAA` 說明 DNS 伺服器會怎麼回答（A 或 AAAA），用 daemon 自己的 DNS 選擇函式。
  - `--json` 輸出穩定、只加欄位的 JSON。
  - 結束碼：0 有答案、1 `ALL_DOWN`、2 `NOT_FOUND` 或用法錯誤、3 `UNAVAILABLE` 或 `INCOMPATIBLE`；`--explain` 的結束碼與不加時相同，失敗時仍印出原因。
- `emfor-client`（`clients/rust`）：把 `lookup` 的「讀 store、判斷 stale、解析記錄、算候選」抽成 lookup 與 explain 共用的前置步驟，新增公開的 explain 函式與可從 store 路徑建立的公開 client 型別；`lookup` 的行為與四個錯誤碼不變。
- `emfor-format`：在 `rules.rs` 的 `healthy_or_all` 旁新增候選層級說明函式（回傳候選來自哪一層、每個 backend 為什麼在或不在），並讓 `healthy_or_all` 改成呼叫它，候選層級的規則只有一份。client 與 DNS 兩條路都用它。
- `emfor-daemon` 新增對 `emfor-client` 的依賴；不會多出任何要安裝的檔案（`emfor-client` 是函式庫，連結進 `emfor`）。
- 文件：中英手冊（第 6 章 client、第 8 章 operations、導讀）、`docs/deploy/daemon.md`、CHANGELOG 的 Unreleased；用語表 `docs/spectra/LANGUAGE.md` 補上「候選層」。
- 順手更正兩處不正確的註解：client `select.rs` 寫「Pure functions」、daemon `select.rs` 寫「No I/O」，但兩者都會推進輪流計數器。
- 隨 0.5.0 之後的下一版發佈，這次不打 tag。

## Non-Goals

- 不做 SRV 的 explain：priority、weight、順序是另一套邏輯（`srv_targets_at`），先讓 A 與 AAAA 通了再說。
- 不改任何選擇行為：候選規則、`balance` 的挑選、DNS 伺服器、daemon 的檢查、store 格式、libemfor 的 C API 與其他語言的 client 都不變。
- 不改 `emfor status`；它回答的是「daemon 發佈了什麼」，不是「我的程式會拿到哪台」。
- explain 不推進任何輪流計數器，也不假裝知道「下一個是誰」：`round-robin`、`weighted`、`locality`、`fastest`、沒有 key 的 `sticky` 只列候選與規則；`failover` 與有 key 的 `sticky` 才給確定的一台。
- 不讓 `--dns` 讀 daemon 的記憶體：它讀 store，所以 reason 剛變化的那一秒可能與真正的 DNS 答案不同，輸出會明講。

## Capabilities

### New Capabilities

- `lookup-command`: `emfor lookup` 子命令的行為：預設的 client 語意、`--explain`、`--json`、`--dns A|AAAA`、結束碼、explain 不改動任何狀態、DNS 模式的限制。

### Modified Capabilities

（無。候選規則的現有要求沒有變，仍然成立；共用的前置步驟屬於內部實作，不改 client 的公開行為。）

## Impact

- 程式：`crates/emfor-format/src/rules.rs`（層級說明）、`clients/rust/src/`（`client.rs`、`select.rs`、`lib.rs`：前置步驟、explain、公開 client）、`crates/emfor-daemon/src/`（`cli.rs` 新子命令、新的 lookup 模組、`select.rs` 的 DNS explain）、`crates/emfor-daemon/Cargo.toml`（新增依賴）。
- Spec：新增 `docs/spectra/specs/lookup-command/spec.md`（隨 archive 產生）。
- 文件：`docs/manual/` 與 `docs/manual.zh_TW/` 的第 6、8 章與導讀、`docs/deploy/daemon.md`、`docs/spectra/LANGUAGE.md`、`CHANGELOG.md`。
- 不影響 store 格式、plugin 協定、daemon 的檢查與 DNS 行為、`libemfor` 與其他語言的 client、FreeBSD port 的安裝清單。
