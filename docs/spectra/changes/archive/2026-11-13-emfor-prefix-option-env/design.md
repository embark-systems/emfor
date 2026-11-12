## Context

`DEFAULT_CONFIG_PATH`（`crates/emfor-daemon/src/config.rs`）目前是 `env!("EMFOR_DEFAULT_CONFIG_PATH")`，值由 `crates/emfor-daemon/build.rs` 算出：build.rs 讀 `EMFOR_PREFIX`、以 `include!("build/prefix.rs")` 呼叫 `default_config_path`，輸出 `cargo:rustc-env` 與 `cargo:rerun-if-env-changed=EMFOR_PREFIX`。`config.rs` 的單元測試 `prefix_to_default_config_path` 以 `#[path = "../build/prefix.rs"]` 引入同一檔案測規則表。

emfor-status 的 design 當時記下「不用 `option_env!` 直接讀完整路徑：只能拿整條字串，無法由 prefix 推算，也無法在編譯時檢查」。本 change 推翻這個決定（Supersedes：emfor-status design「build.rs 由 EMFOR_PREFIX 算出預設設定檔路徑」）。

已在 scratchpad 以 rustc 1.99.1 做原型（實測）：8 種輸入（未設、空字串、`/usr`、`/usr/`、`/usr/local`、`/usr/local/`、`/opt/emfor`、`/`）結果與現行規則一致；`EMFOR_PREFIX=usr/local` 編譯失敗，`error[E0080]` 訊息含 `EMFOR_PREFIX must be an absolute path`；`EMFOR_PREFIX` 由 `/a` 改 `/b` 後下一次 `cargo run` 印出新路徑，dep-info 有 `# env-dep:EMFOR_PREFIX=/b`。

## Goals / Non-Goals

**Goals:**

- 預設設定檔路徑的規則只寫在 `config.rs` 一處，刪掉 build script。
- 規則、`--help` 顯示的預設值、改 `EMFOR_PREFIX` 會重新編譯、相對路徑讓編譯失敗：全部維持。

**Non-Goals:**

- 不改任何使用者可見的路徑或指令行為；capability-level observable behavior 不變。
- 錯誤訊息不帶出錯誤的值（const `panic!` 只接受固定字串）。

## Decisions

### 以 option_env! 加 const fn 在編譯時推算路徑

`config.rs` 新增 `const fn prefix_dir(prefix: Option<&'static str>) -> Result<&'static str, &'static str>`：回傳正規化後的 prefix（未設、空字串、`/`、`/usr` 與帶一個結尾 `/` 的 `/usr/` 為 `""`；其他去掉一個結尾 `/`），相對路徑回傳 `Err("EMFOR_PREFIX must be an absolute path")`。在 `str` 上切片用 `split_at` 或 byte slice 加 `str::from_utf8`，都是 const 可用的 API。

`DEFAULT_CONFIG_PATH` 由 const 區塊組成：`const PREFIX: &str = match prefix_dir(option_env!("EMFOR_PREFIX")) { Ok(p) => p, Err(e) => panic!("{}", e) }`——const 中的 `panic!` 可以把一個 `&str` 原樣傳出，但不能格式化錯誤的值（例如 `{prefix:?}`），所以錯誤訊息只有 `prefix_dir` 回傳的那一句，不含錯誤的值。接著以 `[u8; PREFIX.len() + CONFIG_SUFFIX.len()]` 的 const 陣列逐 byte 複製 prefix 與 `/etc/emfor/emfor.toml`，再以 `str::from_utf8` 轉回 `&str`（`Err` 分支 `unreachable!()`：兩段合法的 `&str` 接起來一定是合法 UTF-8；`option_env!` 本身遇到非 UTF-8 值時就會編譯失敗）。

替代方案：
- 保留 build.rs：可行，但規則分散在兩個檔案、要 `include!` 與 `#[path]` 共用，且多一個 build script 編譯單位。
- `const_format` 之類的套件：只為了拼一個字串就多一個相依，不值得。
- `option_env!` 直接讀完整設定檔路徑：使用者介面要變，不採用。

### 回傳 Result 而非在規則函式內 panic

規則函式回傳 `Result`，`panic!` 只放在組 `DEFAULT_CONFIG_PATH` 的 const 項目：單元測試可以在執行期直接比對 `Err`，不用 `#[should_panic]`。

### 重新編譯靠 rustc 的 env-dep

`option_env!` 讀到的環境變數由 rustc 寫進 dep-info（`# env-dep:EMFOR_PREFIX=...`），Cargo 據此在值改變時重新編譯該 crate。未設時同樣會記錄，從未設改成有值也會重編。

## Implementation Contract

- 刪除後 `crates/emfor-daemon/build.rs` 與 `crates/emfor-daemon/build/` 不存在，`crates/emfor-daemon/Cargo.toml` 不需改（沒有 `build =` 設定）。
- `config::DEFAULT_CONFIG_PATH` 仍是 `pub const &str`，`cli.rs` 的 `default_value = DEFAULT_CONFIG_PATH` 不變。
- 單元測試 `prefix_to_default_config_path` 改測 `config.rs` 的規則函式，表格涵蓋：`None`、`""`、`"/"`、`"/usr"`、`"/usr/"` → `/etc/emfor/emfor.toml`；`"/usr/local"`、`"/usr/local/"` → `/usr/local/etc/emfor/emfor.toml`；`"/opt/emfor"` → `/opt/emfor/etc/emfor/emfor.toml`；`"usr/local"` → `Err`，訊息含 `EMFOR_PREFIX`。測試以執行期字串串接驗證「prefix + 後綴」等於期望值。
- 整合測試 `help_shows_the_built_in_config_path`（`crates/emfor-daemon/tests/daemon.rs`）不改，仍需通過。
- 手動驗證：
  - `EMFOR_PREFIX=usr/local cargo build -p emfor-daemon` 失敗，錯誤含 `EMFOR_PREFIX must be an absolute path`；
  - `EMFOR_PREFIX=/usr/local cargo run -q --bin emfor -- --help` 顯示 `[default: /usr/local/etc/emfor/emfor.toml]`，接著不帶 `EMFOR_PREFIX` 再跑一次，顯示 `[default: /etc/emfor/emfor.toml]`。
- capability-level observable behavior 不變；唯一差異是相對路徑的編譯錯誤訊息不再含錯誤的值。
- 範圍內：`config.rs` 的路徑規則與測試、刪除兩個 build 檔案、`docs/spectra/config.yaml` 的描述、service-config 與 status-command 主 spec 的 trace 註解。
- 範圍外：`EMFOR_PREFIX` 的規則本身、`docs/deploy/daemon.md`（沒有提 build.rs）、client 與 store 的預設值。

## Risks / Trade-offs

- [const 拼字串比 `format!` 冗長] → 集中在一個 const 區塊，附註解說明為何不用 `format!`。
- [錯誤訊息少了錯誤的值] → 訊息仍點名 `EMFOR_PREFIX`，使用者看自己的環境變數即可；已寫進 Non-Goals。
- [舊版 Cargo 不依 env-dep 重編] → 專案用 edition 2024，需要的 Cargo 版本早已支援；手動驗證步驟會確認改值後路徑改變。

## Migration Plan

純編譯期重構，沒有部署步驟。要回復就還原這個 commit。

## Open Questions

（無）
