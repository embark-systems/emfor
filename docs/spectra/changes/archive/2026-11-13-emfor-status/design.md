## Context

daemon 把健康狀態寫進本機 LMDB（`crates/emfor-format` 定義格式）：每台後端有 `state`、`checked_at`、`reason`（最後一次 down / error 的訊息，up 後清空）、`latency_us`；服務有 `balance`、`on_all_down`、`all_down`、`pool`；另有 `meta/heartbeat`（每秒更新，正常關閉後為 `0`）、`meta/started_at`、`meta/site`、`meta/schema_version`。daemon 寫入前對 `<store>.writer` 加 flock，讀者不受影響。client 開 store 前先確認檔案存在，避免由應用程式帳號建立 `-lock` 檔而讓 daemon 之後起不來。

目前沒有任何指令能把這些內容給人看；`emfor` 只有 `--config` 與 `--check-config` 兩個旗標，`DEFAULT_CONFIG_PATH` 寫死為 `/etc/emfor/emfor.toml`。

## Goals / Non-Goals

**Goals:**

- `emfor status` 一眼看出 daemon 是否還在更新、哪個服務沒有健康後端、哪台後端為什麼 down。
- 結束碼可直接給 Nagios、cron、Ansible 判斷。
- `--watch` 讓維運人員在切換或排障時盯著狀態變化。
- 以 `EMFOR_PREFIX` 編譯後，`emfor` 與 `emfor status` 不帶 `--config` 就找得到設定檔。
- 不新增 crate。

**Non-Goals:**

- 狀態歷史、抖動統計（LMDB 只存現況，需要 daemon 另存事件，另開 change）。
- 互動式 TUI（捲動、展開、篩選鍵）與 ratatui 之類的套件。
- 從 status 觸發檢查或修改任何狀態。
- store 預設路徑與 client 的預設值；依目標作業系統決定預設路徑；以完整路徑覆寫設定檔位置的變數。

## Decisions

### status 做成 emfor 的子指令

`emfor status` 用 clap 子指令，`--config` 改為 global 參數，`emfor status --config x` 與 `emfor --config x status` 都可用；不帶子指令的 `emfor` 照舊啟動 daemon，`--check-config` 不變。每台主機本來就裝了 `emfor`，daemon crate 已用 heed 讀寫同一份 LMDB。替代方案是另做執行檔或放進 client crate：client 的 store 模組是私有的、只為查詢設計，另做執行檔則要重複讀取程式，皆不採用。

### 只讀設定檔的 store 表

status 只解析 TOML 並以現有 `Validator` 的 store 規則取出 `StoreConfig`（`path`、`map_size`），其餘表格不驗證、不啟動外掛。這樣在設定檔被改壞、daemon 仍以舊設定執行時，status 照樣能用；服務清單來自 store 而不是設定檔。`[store]` 表本身有錯時結束碼 2。

### 唯讀開啟 LMDB 且先確認檔案存在

以 heed `EnvFlags::READ_ONLY | NO_SUB_DIR` 開啟，`map_size` 用設定檔的值；開啟前先 `metadata` 確認檔案存在，與 client 相同，避免建立 `-lock` 檔。一次讀交易讀出 meta 與全部 `svc/` 記錄，畫面內容來自同一個時間點。

### 心跳過期門檻固定 3 秒

與 client 的 `max_staleness` 預設相同（`clients/rust/src/config.rs` 的 `DEFAULT_MAX_STALENESS_MS`）。不提供選項：status 判斷的是「daemon 有沒有在跑」，與 client 個別調整的容忍度無關。

### 輸出由純函式產生

讀取（I/O）與呈現分開：`Snapshot`（meta 加服務記錄）與目前時間進入純函式，產出文字行、JSON 值與結束碼；`--watch` 的比較也是純函式（前後兩個 `Snapshot` → 要反白的列與狀態轉換）。格式、結束碼與轉換都能用單元測試涵蓋，不必依賴終端機。

### watch 用 ANSI 與 tokio 處理中斷

`--watch` 進入 alternate screen、隱藏游標，每秒重畫；以 tokio `signal::ctrl_c` 收到中斷後恢復終端機再以 0 結束。反白用 reverse video。是否為終端機以標準庫 `IsTerminal` 判斷；`NO_COLOR` 或非終端機時不輸出任何跳脫字元（`--watch` 本身要求終端機）。不引入 crossterm / ratatui。

### build.rs 由 EMFOR_PREFIX 算出預設設定檔路徑

`crates/emfor-daemon/build.rs` 讀 `EMFOR_PREFIX`，輸出 `cargo:rerun-if-env-changed=EMFOR_PREFIX` 與 `cargo:rustc-env=EMFOR_DEFAULT_CONFIG_PATH=<path>`；`config.rs` 的 `DEFAULT_CONFIG_PATH` 改為 `env!("EMFOR_DEFAULT_CONFIG_PATH")`。前綴轉路徑的規則寫在一個獨立檔案的函式裡，build.rs 以 `include!` 使用，daemon 的單元測試以 `#[path]` 引入同一檔案測試規則表。不用 `option_env!` 直接讀完整路徑：只能拿整條字串，無法由 prefix 推算，也無法在編譯時檢查。不依目標作業系統調整預設值：要 `/usr/local/etc` 就以 `EMFOR_PREFIX=/usr/local` 編譯。

## Implementation Contract

**行為**

- `emfor status [--config PATH] [--service NAME] [--json | --watch]`。
- 文字輸出順序與格式依 spec「Status text output」：`store: <path>`、daemon 行、各服務行（名稱排序）與其後端行（store 順序）。年齡格式 `0.8s` / `1m01s` / `3h12m` / `2d03h`，延遲 `1.2ms` / `1.4s`，沒有值時 `-`，從未檢查為 `never`；`fastest` 的快組成員標 `pool`。欄位以兩個空白分隔並依各欄最長值對齊。
- JSON：`{"store","daemon","heartbeat_ms","heartbeat_age_ms","started_at_ms","site","services"}`，`services` 為 store 中的服務記錄原樣（`ServiceRecord` 的 serde 形狀），名稱排序。
- daemon 狀態：heartbeat `0` 為 `stopped`；`now - heartbeat > 3000 ms` 為 `stale`；其他為 `running`。heartbeat 在未來（時鐘倒退）視為年齡 0。
- 結束碼：2＝設定檔讀不到或 `[store]` 有錯、store 不存在或開不了、`meta/schema_version` 不是 `1`、`--service` 找不到；1＝daemon `stale` / `stopped`，或顯示的服務中有 `all_down`；0＝其他。
- `--watch`：每秒重畫；反白只看後端 `state`、`reason` 與服務 `all_down` 的變化；底部最近 10 筆轉換（新的在上），格式 `HH:MM:SS <service> <backend id> <old> -> <new>` 與 `HH:MM:SS daemon <old> -> <new>`，時間用本機時區；讀取失敗時顯示錯誤行並在下一秒重試；Ctrl-C 恢復終端機後以 0 結束；stdout 不是終端機時印 `--watch needs a terminal` 並以 2 結束；`--watch` 與 `--json` 由 clap 設為互斥。
- `EMFOR_PREFIX`：未設、空字串或 `/usr` → `/etc/emfor/emfor.toml`；其他絕對路徑（去掉一個結尾 `/`）→ `<prefix>/etc/emfor/emfor.toml`；非絕對路徑 → build.rs `panic!`，訊息含 `EMFOR_PREFIX`。

**失敗訊息**

- `cannot open store <path>: no such file`
- `store format version <n>; this emfor reads version 1`
- `service <name> not found`
- `--watch needs a terminal`
- 設定檔讀不到或 `[store]` 有錯：沿用 config 模組的錯誤文字（含 TOML 路徑）。

**驗收**

- 單元測試：年齡與延遲格式表、daemon 狀態判斷、結束碼表（spec 範例六列）、`--service` 篩選、JSON 欄位、watch 比較（反白與轉換、年齡變化不反白、最多 10 筆）、`EMFOR_PREFIX` 規則表六列。
- `crates/emfor-daemon/tests/daemon.rs`：對執行中的 daemon 跑 `emfor status`（結束碼 0、輸出含服務與 `daemon: running`）、`--json --service`、daemon 停止後結束碼 1 且 `daemon: stopped`、store 不存在時結束碼 2 且未建立 `-lock`、設定檔其他部分有錯仍可讀、`emfor status --watch` 輸出導向檔案時結束碼 2、`emfor --help` 顯示預設設定檔路徑。
- 手動：在終端機跑 `emfor status --watch`，停掉一台後端確認反白與轉換清單，Ctrl-C 後終端機恢復；`EMFOR_PREFIX=usr/local cargo build -p emfor-daemon` 失敗並說明原因。

**範圍**

- 範圍內：daemon crate 的 `status` 子指令與 `status.rs`、`build.rs`、`DEFAULT_CONFIG_PATH`、文件。
- 範圍外：store 格式、client、DNS、檢查器行為；Non-Goals 所列項目。

## Risks / Trade-offs

- [status 與 daemon 用不同 `map_size` 開同一檔案] → 唯讀環境下 LMDB 以檔案實際大小為準；仍以設定檔的值開啟，與 daemon 一致。
- [設定檔被改成另一個 store 路徑但 daemon 尚未重啟] → status 讀新路徑，可能顯示「store 不存在」；daemon 的 store 路徑本來就要重啟才生效，文件註明。
- [watch 每秒開一次讀交易] → 一個讀交易與幾 KB 的 JSON，成本可忽略（估計，以 `--watch` 跑數分鐘觀察 CPU 確認）。
- [`EMFOR_PREFIX` 只影響 daemon，`cargo install --root` 不會傳給 build.rs] → 文件寫明要另外設定環境變數。
- [時鐘倒退讓心跳看起來在未來] → 年齡當 0，視為 running。
