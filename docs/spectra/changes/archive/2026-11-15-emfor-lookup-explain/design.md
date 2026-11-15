## Context

今天只有兩種方法知道一個查詢會拿到哪台 backend：用 client 函式庫寫一段程式，或對 daemon 的 DNS 做查詢。兩者都看不到原因。選擇邏輯分散在三處（`/spectra-discuss` 於 2026-11-15 用 graphify 圖與原始碼核對過，Haiku 做第一輪，其餘由主對話讀程式確認）：

- 候選層級的規則在 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all`（primary 與 backup、通過檢查與失敗中、`on_all_down` 為 `open` 時的 `unknown`、最後是全部），client 與 daemon 的 DNS 都呼叫它。
- 「挑一台」各寫一份：client 在 `clients/rust/src/select.rs`（`candidates`、`pool_candidates`、`pick_at`、`cumulative_index`、`rendezvous_index`），daemon 在 `crates/emfor-daemon/src/select.rs`（`select_at`、`smooth_weighted`、`srv_targets_at`）。`weighted` 的演算法兩邊不同：client 是累積權重輪盤，daemon 是 smooth weighted round-robin。
- client 的 `pick_at` 把輪流計數器做成參數 `next: impl FnOnce() -> u64`，真正推進計數器的是 `Client::next`（程序內記憶體，起點由 `RandomState` 隨機）；`candidates`、`pool_candidates` 只吃資料。
- DNS 在選擇前先依 A 或 AAAA 過濾 family，再算層級，永遠以 `trust_health = true` 處理（daemon 的 store 永遠是新的）；`sticky` 沒有 key，退化成 `round-robin`。
- `clients/rust` 與 `crates/emfor-daemon` 都依賴 `emfor-format`；`emfor-daemon` 目前不依賴 `emfor-client`，`emfor status` 也沒有用到任何選擇函式。
- 已驗證的事實：daemon 的公開 `select::candidates` 在 production 沒有呼叫者（只有 `select.rs` 自己的測試）；FreeBSD 的 `pthread_atfork` 處理函式只在第一次經過 `global()`（`emfor_client::lookup`）時註冊，不是連結時（`clients/rust/src/lib.rs` 的 `global()` 呼叫 `fork::register()`）；`Cargo.lock` 只有一個 `lmdb-master-sys`（0.2.6），由 `emfor-client` 與 `heed` 共用，`clients/rust/tests/same_lmdb.rs` 檢查這一點。這些是讀原始碼與 `Cargo.lock` 的結果，沒有執行測試。

`docs/compatibility.md` 把文字輸出列為不穩定、`--json` 列為穩定（只加欄位）。

## Goals / Non-Goals

**Goals:**

- 一個不需要 client 函式庫的指令，回答「我的程式現在會拿到哪一台，為什麼」。
- explain 與真正的查詢走同一條程式路徑，不會各說各話。
- DNS 的 A 與 AAAA 也能說明。

**Non-Goals:**

- 不做 SRV 的 explain、不改任何選擇行為、不改 DNS 伺服器、daemon 的檢查、store 格式與 `libemfor`。
- 不讓 explain 推進任何計數器，也不假裝知道「下一個是誰」。
- 不改 `emfor status`。

## Decisions

### 新子命令放在 `emfor` 二進位檔，用 `emfor-client` 的函式

`emfor lookup` 是 `emfor` 的子命令，與 `status`、`server` 並列。它在內部呼叫 `emfor-client` 的函式：讀 store、判斷 stale、算候選、挑選，都是 client 本來就有的程式碼，所以預設輸出就是「一個新程序用 client 函式庫會得到的答案」。`emfor-daemon` 新增對 `emfor-client` 的依賴。`emfor-client` 是函式庫，連結進 `emfor`，不會多出要安裝的檔案；FreeBSD port 的 `PLIST_FILES` 仍然是 `bin/emfor` 與 `bin/emfor-check`。

替代方案：把 `candidates`、`pool_candidates`、`pick_at` 等整批搬進 `emfor-format`，daemon 的 CLI 用 `status` 的讀取路徑。依賴不變，但要搬一個有大量單元測試的檔案，改動範圍大；而且 CLI 會有兩套讀 store 的程式。拒絕。

### 抽出共用的前置步驟，讓 lookup 與 explain 走同一條路

`Client::lookup_once` 目前是一個長函式：讀 store、檢查 schema 與 heartbeat、判斷 stale、解析記錄、算候選、挑一台。把前面到「算出候選」的部分抽成一個內部步驟，`lookup` 與新的 `explain` 都呼叫它，之後各自接「挑一台」與「說明」。`lookup` 的行為與四個錯誤碼完全不變。為了讓 `emfor` 能從 store 路徑建立 client，`emfor-client` 公開 `Client`（建構函式 `Client::new(path, max_staleness_ms)` 已存在，只是 `client` 模組是私有的），並新增 `Client::explain`。`libemfor` 的 C API 與其他語言的 client 不受影響。

### 候選層級說明放在 `emfor-format`，`healthy_or_all` 改成呼叫它

在 `rules.rs` 新增一個函式，回傳候選來自哪一層，以及每個 backend 為什麼在或不在候選裡。層級依序是：通過檢查的 primary、通過檢查的 backup、失敗中的 primary、失敗中的 backup，然後（只有 `on_all_down` 為 `open`）通過檢查的 `unknown`、全部 `unknown`、全部 backend。每個 backend 的原因至少包含：`unhealthy`、`unknown`、`last check failed`、被備援層擋住、不在快組、不在本機站台、慢啟動第幾步。`healthy_or_all` 改寫成呼叫這個函式並只取候選，所以層級規則只有一份；client 與 DNS 兩條路都用它。DNS 先依 family 過濾，再把過濾後的清單交給它（與現在的順序相同）。

### explain 不推進任何狀態

`pick_at` 的計數器本來就是參數：explain 傳一個固定值的閉包，或根本不呼叫它。結果依 `balance` 分兩類：

- `failover`，以及有 key 的 `sticky`：給確定的那一台（`candidates[0]`，或加權 rendezvous 雜湊的最高分）。
- `round-robin`、`weighted`、`locality`、`fastest`、沒有 key 的 `sticky`、不認得的 `balance`：列出候選與規則（`weighted` 寫成權重比例，`locality` 寫成站台子集，`fastest` 寫成快組，慢啟動時寫成有效權重），並明講「實際挑選取決於程序內的輪流起點，新程序的起點是隨機的」。

沒有 `--explain` 的 `emfor lookup` 仍然印出一個位址，等於一個新程序會看到的。需要穩定結果的 cron 工作要用 `failover`，或 `sticky` 加 `--key`；手冊要寫明。

### DNS 模式只做 A 與 AAAA，讀 store

`--dns A|AAAA` 用 daemon 的 `select.rs` 函式（family 過濾在層級之前、`trust_health = true`、`weighted` 用 smooth weighted round-robin、`sticky` 沒有 key 時當 `round-robin`）。它讀 store，不讀 daemon 的記憶體：reason 的變更最多晚約一秒才寫進 store，所以在變化後的那一秒，explain 可能與 DNS 伺服器真正的答案不同，輸出會明講。DNS 的輪流計數器在 daemon 記憶體，CLI 看不到，所以和 client 模式一樣只能列候選與規則。SRV 不做：priority、weight 與順序是另一套邏輯。

替代方案：直接對 daemon 的 DNS 埠送一個查詢。得到的是真實答案，但看不到候選層級與原因，而且 daemon 沒開 DNS 時無法使用。拒絕。

### 輸出與結束碼

文字輸出不承諾穩定。`--json` 穩定且只加欄位。結束碼：0 有答案、1 `ALL_DOWN`、2 `NOT_FOUND` 或用法錯誤、3 `UNAVAILABLE` 或 `INCOMPATIBLE`。`--explain` 的結束碼與不加時相同，失敗時仍然印出原因與已知的候選資訊。store 路徑與 `emfor status` 相同：由 `--config` 指定的設定檔的 `[store] path` 決定；staleness 沿用 client 的環境變數 `EMFOR_MAX_STALENESS`（預設 3 秒）。

## Implementation Contract

**Behavior**

- `emfor lookup <service>` 印出一行 `ip:port`（IPv6 為 `[ip]:port`）並以 0 結束，答案與 client 函式庫的 `lookup` 在同一份 store 上相同（輪流類的 `balance` 例外，見上）。
- `--key <k>` 只對 `sticky` 有意義，與 client 的 `affinity_key` 相同；空字串視為沒有 key。
- `--explain` 在答案之前或之後印出說明：store 的路徑、heartbeat 的年齡、是否 stale（以及 `EMFOR_MAX_STALENESS`）、服務的 `balance` 與 `on_all_down`、候選來自哪一層、每個 backend 在或不在候選裡的原因、挑選規則與是否確定。
- `--dns A|AAAA` 以 daemon 的語意說明：只看該 family 的 backend、store 一律視為 fresh、`weighted` 以 smooth weighted round-robin 說明、沒有 key。沒有該 family 的 backend 時，說明「這個服務沒有 A（或 AAAA）」，結束碼 1（沒有答案）。
- `--json` 輸出一個物件，欄位只增不減。草案：`service`、`via`（`client` 或 `dns`）、`family`（`null`、`A`、`AAAA`）、`store`（`path`、`heartbeat_age_ms`、`stale`）、`balance`、`on_all_down`、`result`（`status` 為 `ok` 或 `error`，`address`，`error` 含 `code` 與 `message`）、`explain`（`tier`、`candidates`、`excluded` 各含 `id` 與 `reason`、`selection` 含 `rule`、`deterministic`、`chosen`）。沒有 `--explain` 時沒有 `explain` 欄位。

**Interface / data shape**

- 新增子命令與選項如上；`emfor --help` 列出它。
- `emfor-format`：新的層級說明函式與其回傳型別（層級列舉與每個 backend 的原因），`healthy_or_all` 簽名不變。
- `emfor-client`：公開 `Client`（`new`、`lookup`、`explain`）；`lookup`、`ErrorCode` 與全域 `emfor_client::lookup` 不變。
- `emfor-daemon`：新模組處理 `lookup` 子命令與 DNS 模式的說明，使用 `select.rs` 的 family 與候選函式。

**Failure modes**

- store 讀不到、版本不符：結束碼 3，訊息與 client 的錯誤相同。
- 服務不存在：結束碼 2。
- `closed` 的服務沒有 healthy backend，或 stale 時的 `closed`：結束碼 1；`--explain` 仍印出層級與原因。
- `--dns` 的值不是 `A` 或 `AAAA`：用法錯誤，結束碼 2。
- explain 永遠不寫 store、不推進任何輪流計數器。

**Acceptance criteria**

- `emfor-format` 的層級說明函式對現有候選規則表的每一列給出與 `healthy_or_all` 相同的候選（單元測試逐列比對）。
- 對同一份 store，`emfor lookup` 的答案與 `emfor-client` 的 `lookup` 相同（`failover`、有 key 的 `sticky` 確定；輪流類的檢查答案落在 explain 列出的候選裡）。
- explain 之後，同一個 client 的下一次 `lookup` 的輪流序列與沒有呼叫 explain 時相同（測試）。
- `--json` 的欄位名稱與型別有測試；結束碼表有測試。
- DNS 的範例：同一份 store，A 與 AAAA 的候選層級不同時，說明各自正確。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 在 Debian docker 映像 `emfor-clients-test` 通過；macOS 與 FreeBSD 等發版前再測。

**Scope boundaries**

- 範圍內：`emfor lookup`、`emfor-client` 的前置步驟與 explain、`emfor-format` 的層級說明、DNS 的 A 與 AAAA 說明、手冊（第 6、8 章與導讀，兩種語言）、`docs/deploy/daemon.md`、`CHANGELOG.md`、用語表。
- 範圍外：SRV、選擇行為、DNS 伺服器、daemon 的檢查、store 格式、`libemfor` 與其他語言的 client、`emfor status`。

## Risks / Trade-offs

- `emfor-daemon` 連結 `emfor-client` 後，同一個二進位檔裡有兩套讀 LMDB 的程式碼（daemon 的 heed 與 client 直接用的 `lmdb-master-sys`）。`Cargo.lock` 只有一個 `lmdb-master-sys`，由 `same_lmdb.rs` 檢查；但一個程序裡兩個 LMDB 環境開同一個 store 的行為，這個 change 的 `emfor lookup` 不會遇到（它只開 client 的環境，不經過 daemon 的 heed）。仍要在實作時確認 `emfor lookup` 的程序裡沒有第二個環境。
- FreeBSD 的 `pthread_atfork` 處理函式只在 `global()` 第一次被呼叫時註冊，連結本身不會註冊。`emfor lookup` 直接建立 `Client`，不經過 `global()`，所以也不註冊；`emfor server` 不呼叫 client。因此這個風險很低，但要加一個測試或檢查確認 `emfor server` 的程序沒有因此註冊處理函式。
- 層級說明函式取代 `healthy_or_all` 的內部實作，是這個 change 最敏感的改動：`healthy_or_all` 被 client 與 DNS 共用，任何差異都會改變查詢結果。以現有的候選規則表與 conformance 資料（`tests/conformance/v1/cases.json`）逐列比對來守住。
- `weighted` 的兩種演算法在 explain 的說明裡不同（client 的累積權重輪盤、daemon 的 smooth weighted round-robin）；explain 要明講是哪一種，不能讓使用者以為兩邊的順序相同。
- DNS 模式讀 store，與 daemon 記憶體最多差約一秒（reason 的寫入延遲，依手冊）；這是設計上的限制，不是 bug，輸出會明講。
- 加了對 `emfor-client` 的依賴後，`emfor` 的二進位檔會變大一些，大小增加未量測。
