## Why

目前要知道各服務的健康狀況只能用 `dig` 查 DNS 或翻 daemon 的 log，看不到哪台後端 down、為什麼 down、daemon 是否還在更新。LMDB 裡已經有每台後端的狀態、最後一次失敗原因、檢查時間與延遲，只缺一個讀出來給人看的指令；而設定檔預設路徑寫死在 `/etc/emfor/emfor.toml`，裝在其他 prefix 的主機每次都得帶 `--config`。

## What Changes

- `emfor status` 子指令：唯讀開啟 store，先顯示 daemon 狀態（store 路徑、心跳經過時間、執行時間、`site`），再依服務列出 `balance`、`on_all_down`、是否全部不健康，以及每台後端的健康狀態、距上次檢查時間、檢查延遲、最後一次失敗原因；`fastest` 服務標出快組成員。store 路徑取自 `--config` 指定的設定檔的 `[store] path`，未設定時為 `/var/lib/emfor/state.mdb`。
- `emfor status --service <name>` 只顯示一個服務；`--json` 輸出機器可讀的 JSON。
- 結束碼：daemon 心跳過期（超過 3 秒或為 0）或有服務全部不健康為 1；讀不到設定檔、store 不存在或無法開啟、格式版本不符為 2；其他為 0。
- `emfor status --watch`：在終端機上每秒重讀並重畫，與上一個畫面相比有變化的列反白，畫面底部列出本次 `--watch` 期間看到的最近 10 筆健康狀態轉換；輸出不是終端機時拒絕執行。
- 設定檔預設路徑在編譯時決定：daemon crate 的 `build.rs` 讀環境變數 `EMFOR_PREFIX`，未設定或為 `/usr` 時為 `/etc/emfor/emfor.toml`，其他為 `<prefix>/etc/emfor/emfor.toml`；不是絕對路徑時編譯失敗。`emfor --help` 顯示編進去的預設值。
- 不帶子指令的 `emfor` 照舊啟動 daemon，`--config`、`--check-config` 的行為不變。

## Capabilities

### New Capabilities

- `status-command`: `emfor status` 的輸出內容、篩選與 JSON、結束碼、`--watch` 動態顯示，以及唯讀開啟 store 的規則。

### Modified Capabilities

- `service-config`: 設定檔預設路徑改為編譯時由 `EMFOR_PREFIX` 決定（未設定時仍為 `/etc/emfor/emfor.toml`）。

## Impact

- 程式：crates/emfor-daemon/src/cli.rs（子指令）、新增 crates/emfor-daemon/src/status.rs、crates/emfor-daemon/src/config.rs（`DEFAULT_CONFIG_PATH`）、新增 crates/emfor-daemon/build.rs。
- 測試：crates/emfor-daemon/tests/daemon.rs（對執行中的 daemon 跑 `emfor status`）、status.rs 單元測試。
- 文件：新增 docs/deploy/daemon.md（以 `EMFOR_PREFIX` 編譯與安裝、`emfor status` 用法）、examples/rc.d/emfor 註解、docs/spectra/config.yaml。
- 相依：不新增 crate；終端機控制用 ANSI 跳脫字元，是否為終端機用標準庫 `IsTerminal` 判斷。
- 不影響 store 格式、client 與 DNS。
