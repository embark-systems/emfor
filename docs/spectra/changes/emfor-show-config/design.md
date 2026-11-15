## Context

`emfor-conf-d` 之後，`config::read_layers`（`crates/emfor-daemon/src/config.rs`）回傳 `Layers`：合併後的 `table`（`toml::Table`）、`origins`（鍵路徑，格式同 `key_path`，對最後設定它的檔案，只在設了 `include_dir` 時才有內容）、`files`、`warnings` 與 `included`。`emfor check-config` 在 `crates/emfor-daemon/src/cli.rs` 的 `start` 裡走 `check_only` 分支：載入、啟動並停止 plugin、印 `config ok: ...`，設了 `include_dir` 時再印 `read <path>`。以上是讀原始碼的結果，沒有執行測試。

- `toml::Table` 在這個專案的設定下以鍵排序（`BTreeMap`），輸出順序因此固定；`toml::Value` 的 `Display` 可以輸出內嵌的 TOML 值（字串、數字、布林、陣列、內嵌表）。
- 設定只接受密碼檔的路徑：手冊第 4 章寫明 `password_file`「never the password itself」，`secret_file` 與 `previous_secret_file` 也是路徑；`checker` 目錄與 `config.rs` 裡沒有接受內嵌密碼的鍵（grep `password|secret|token` 的結果）。plugin 的 `command` 是任意 argv，使用者可以自己把祕密寫進去，這點與設定檔本身一樣。

## Goals / Non-Goals

**Goals:** 一個指令印出合併後的設定與每個值的來源檔案；輸出可以餵回 `emfor check-config`；不加旗標的輸出不變。

**Non-Goals:** 見 proposal：不套用預設值、不遮蔽、不改 daemon。

## Decisions

### 旗標與輸出的分工

`emfor check-config --show`。載入、驗證、plugin 的啟動與停止、錯誤輸出與結束碼都與不加旗標時相同；無效的設定照舊把錯誤印到標準錯誤、結束碼 2，標準輸出為空。有效時，標準輸出只有合併後的 TOML，`config ok: ...` 與 `read <path>` 行改寫到標準錯誤。這樣 `emfor check-config --show > merged.toml` 得到的就是一份可直接使用的檔案；若 `config ok` 行留在標準輸出，這個檔案開頭會是非 TOML 的內容。不加 `--show` 時兩者都照舊在標準輸出。

替代方案：獨立的子指令 `emfor show-config`。拒絕：它得重做 `check-config` 已有的載入與驗證，而「輸出的設定一定是驗證過的」是這個功能的前提；一個旗標就夠。

### 輸出格式

由 `config.rs` 的一個函式把 `Layers` 轉成字串，不用 `toml::to_string`（它不帶註解）：

- 先印頂層的純量與陣列，再依序印每個子表；子表標頭用 `[<key_path>]`（鍵路徑格式與驗證錯誤相同，例如 `[services."api.svc.emfor"]`、`[services."api.svc.emfor".check_options]`），表內先印純量與陣列，再印子表。這樣的順序一定是合法的 TOML。
- 值用 `toml::Value` 的 `Display`；陣列裡的表（`backends = [{ addr = ... }]`）以內嵌表輸出，不拆成 `[[...]]`。
- 設了 `include_dir` 時，每一行 `key = value` 後面加 `# <檔案>`，檔案是 `origins` 裡該鍵路徑對應的檔案（與 `read <path>` 顯示的路徑相同）；`origins` 對整個陣列只記一筆，所以註解掛在那一行。表標頭不加註解，因為一個表可能由好幾個檔案合成，標頭的來源沒有意義。
- 沒設 `include_dir` 時只在第一行輸出 `# merged from <主檔路徑>`，其餘不加註解。
- 去掉所有 `#` 註解後的內容解析出來的 `toml::Table`，必須等於 `Layers::table`；測試以這個性質守住（包含含有點的服務名稱、內嵌表、陣列、字串裡有 `#` 的值）。

### 不遮蔽

見 Context：設定只接受密碼檔的路徑，所以沒有需要遮蔽的值；`command` 照原樣印。遮蔽會讓輸出不再能餵回 `check-config`，也沒有可靠的偵測規則，所以不做；手冊寫明。

### 沒有預設值

內建預設值（`interval = "2s"` 等）不出現。要看生效的值，仍是讀手冊第 3 章的預設值表，或另做一個「套用預設值之後」的輸出（非目標）。

## Implementation Contract

**Behavior**

- `emfor check-config --show --config <path>`：設定有效時，標準輸出是合併後的 TOML（格式如上），標準錯誤是平常的 `config ok: ...`（以及設了 `include_dir` 時的 `read <path>` 行），結束碼 0。設定無效時，標準輸出為空，標準錯誤是與不加旗標時相同的錯誤，結束碼 2。
- 不加 `--show`：輸出與現在逐位元組相同。
- 設了 `include_dir`：每個 `key = value` 行尾有 `# <檔案>`；沒設：只有開頭一行 `# merged from <檔案>`。

**Interface / data shape**

- `Command::CheckConfig` 新增旗標 `--show`（布林）。
- `config.rs`：一個把 `&Layers` 轉成 `String` 的函式（輸出如上）。

**Failure modes**

- 載入或驗證失敗：同 `check-config`，沒有新的失敗模式。標準輸出被關閉（broken pipe）：不特別處理，與 `check-config` 現有的 `println!` 行為一致。

**Acceptance criteria**

- 單元測試：輸出去掉註解後解析出的 table 等於 `Layers::table`（涵蓋鍵含 `.`、內嵌表、陣列、字串含 `#`）；設了 `include_dir` 時每個值的註解是最後設定它的檔案（後面的檔案蓋掉前面的、列表被取代）；沒設時只有開頭一行。
- 整合測試：`--show` 的標準輸出餵回 `emfor check-config --config` 通過；無效設定時標準輸出為空、結束碼 2；不加 `--show` 的輸出不變（現有的 `check_config_*` 整合測試不修改仍通過）。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 在 Debian docker（`tools/docker-test.sh`）通過；`tools/check-manual-links.py` 通過；macOS 與 FreeBSD 等發版前再測。

**Scope boundaries**

- 範圍內：`check-config --show`、`config.rs` 的輸出函式、規格、手冊第 3、8 章（中英）、`docs/deploy/daemon.md`、CHANGELOG。
- 範圍外：預設值、遮蔽、獨立子指令、daemon 與 `status` 的行為、輸出格式的長期穩定。

## Risks / Trade-offs

- **註解的歸屬是盡力而為**：兩個檔案各設了同一個表裡的不同鍵時，每個鍵有自己的來源；但一個陣列整體被取代，註解只能指向最後設定它的檔案。手冊要說明。
- **輸出格式不承諾穩定**：只承諾去掉註解後是合法的 TOML 且等於合併結果；腳本不該解析註解。
- **`command` 內的祕密**：見上，風險與設定檔本身相同。
