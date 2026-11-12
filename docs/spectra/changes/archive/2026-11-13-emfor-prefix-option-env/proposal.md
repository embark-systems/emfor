## Why

`emfor` 的預設設定檔路徑目前由 build script 算出：`crates/emfor-daemon/build.rs` 讀 `EMFOR_PREFIX`，以 `include!` 引入 `build/prefix.rs` 的規則，再用 `cargo:rustc-env` 交給 `env!`；單元測試另以 `#[path]` 引入同一檔案。emfor-status 當時不用 `option_env!`，理由是「只能拿整條字串，無法由 prefix 推算，也無法在編譯時檢查」。現在的 Rust（edition 2024，rustc 1.99）const fn 已能在編譯時推算路徑並讓編譯失敗，這個理由不再成立；改用 `option_env!` 可以拿掉整個 build script 與兩處檔案共用的寫法，規則只留在 `config.rs` 一處。

## What Changes

- `crates/emfor-daemon/src/config.rs`：`DEFAULT_CONFIG_PATH` 改由 `option_env!("EMFOR_PREFIX")` 經 const fn 在編譯時算出，規則不變：未設、空字串、`/`、`/usr`（各自可帶一個結尾 `/`）為 `/etc/emfor/emfor.toml`；其他絕對路徑去掉一個結尾 `/` 後接 `/etc/emfor/emfor.toml`；相對路徑讓編譯失敗，錯誤訊息含 `EMFOR_PREFIX`。
- 刪除 `crates/emfor-daemon/build.rs` 與 `crates/emfor-daemon/build/prefix.rs`；`EMFOR_PREFIX` 改變後的重新編譯改由 rustc 寫進 dep-info 的 `env-dep` 觸發，不再需要 `cargo:rerun-if-env-changed`。
- 單元測試 `prefix_to_default_config_path` 改測 `config.rs` 內的規則函式，規則表六列不變。
- `docs/spectra/config.yaml` 的專案描述不再提 build.rs；`docs/spectra/specs/service-config/spec.md` 與 `docs/spectra/specs/status-command/spec.md` 的 trace 註解移除已刪除的兩個檔案。

## Non-Goals

- 不改預設路徑規則、不改 `--help` 顯示、不改 store 與 client 的預設值。
- 不讓錯誤訊息帶出錯誤的值：const 求值中的 `panic!` 只能用固定字串，訊息由現在的 `EMFOR_PREFIX must be an absolute path, got "usr/local"` 變成 `EMFOR_PREFIX must be an absolute path`。spec 只要求錯誤訊息提到 `EMFOR_PREFIX`，仍然符合。
- 不採用另一種寫法「`option_env!` 直接讀完整設定檔路徑」：使用者要的是 prefix，不是完整路徑。

## Impact

- Affected specs: none
- Affected code:
  - New: (none)
  - Modified: crates/emfor-daemon/src/config.rs, docs/spectra/config.yaml, docs/spectra/specs/service-config/spec.md, docs/spectra/specs/status-command/spec.md
  - Removed: crates/emfor-daemon/build.rs, crates/emfor-daemon/build/prefix.rs
- Compatibility: no capability-level observable behavior changes（相對路徑的編譯錯誤訊息不再含錯誤的值）
