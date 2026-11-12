## 1. 以 option_env! 推算預設設定檔路徑

- [x] 1.1 `crates/emfor-daemon/src/config.rs` 新增 const fn 規則函式（依 design「回傳 Result 而非在規則函式內 panic」：回傳正規化 prefix 或 `Err`，訊息含 `EMFOR_PREFIX`，`panic!` 只在組 `DEFAULT_CONFIG_PATH` 的 const 項目），`DEFAULT_CONFIG_PATH` 改由 `option_env!("EMFOR_PREFIX")` 經該函式與 const 區塊組成；單元測試 `prefix_to_default_config_path` 改測新函式，涵蓋 design「Implementation Contract」列出的 9 種輸入。驗證：先改測試並確認它因新函式不存在而編譯失敗，再實作；`cargo test -p emfor-daemon --lib prefix_to_default_config_path` 通過，`help_shows_the_built_in_config_path` 通過。
- [x] 1.2 [after: 1.1] 刪除 `crates/emfor-daemon/build.rs` 與 `crates/emfor-daemon/build/prefix.rs`，`config.rs` 不再有 `#[path = "../build/prefix.rs"]` 與 `env!("EMFOR_DEFAULT_CONFIG_PATH")`。驗證：`git grep -n "EMFOR_DEFAULT_CONFIG_PATH\|build/prefix" -- crates` 沒有結果，`cargo build -p emfor-daemon` 成功。

## 2. 文件與 trace

- [x] 2.1 [after: 1.2] `docs/spectra/config.yaml` 的專案描述改為「設定檔預設路徑在編譯時由 `option_env!` 讀 `EMFOR_PREFIX` 決定」，不再提 build.rs；`docs/spectra/specs/service-config/spec.md` 與 `docs/spectra/specs/status-command/spec.md` 的 trace 註解移除 `crates/emfor-daemon/build.rs` 與 `crates/emfor-daemon/build/prefix.rs`，需求內文不動。驗證：`git grep -n "build.rs\|build/prefix.rs" -- docs/spectra/config.yaml docs/spectra/specs` 沒有結果，`git diff docs/spectra/specs` 只有刪除 trace 行。

## 3. 驗證

- [x] 3.1 [after: 2.1] 手動確認編譯期行為：`EMFOR_PREFIX=usr/local cargo build -p emfor-daemon` 失敗且錯誤含 `EMFOR_PREFIX must be an absolute path`；`EMFOR_PREFIX=/usr/local cargo run -q --bin emfor -- --help` 顯示 `[default: /usr/local/etc/emfor/emfor.toml]`，之後不帶 `EMFOR_PREFIX` 再跑一次顯示 `[default: /etc/emfor/emfor.toml]`（證明改值會重新編譯）。
- [x] 3.2 [after: 3.1] macOS 與 Debian 映像 `emfor-clients-test` 都跑 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，全部通過。
