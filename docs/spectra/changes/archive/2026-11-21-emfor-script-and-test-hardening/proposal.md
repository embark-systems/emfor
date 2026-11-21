## Why

四個低嚴重度項目（2026-11-21 的 Fable 審查，位置已逐一讀過確認）：

1. **flaky 測試。** `crates/emfor-daemon/src/peers/watch.rs` 的 `removed_directory_is_a_failure` 在 `fs::remove_dir_all` 之後 `assert!(notified(&w).await)`，再立刻 `assert!(w.failure().is_some())`。inotify 先送「檔案被刪」的事件，`wait` 對任何事件都返回 `true`，而目錄被刪造成的失敗要在稍後的讀取才記錄；測試的第二個斷言可能在那之前執行。在負載下（完整的 Debian gate）5 次中失敗過 2 次，單獨跑都通過。
2. **`fuzz.yml` 的輸入插值。** `.github/workflows/fuzz.yml` 的 `Fuzz` 步驟把 `${{ github.event.inputs.seconds || 300 }}` 直接放進 `run:` 的 shell 腳本。只有有寫入權的人能手動觸發，風險低，但這是 script injection 的標準樣式。這是所有 workflow 裡唯一把事件資料插進 `run:` 的地方。
3. **pf 範例沒有驗證地址。** `examples/pf/emfor-pf-table.sh` 從 SRV 目標名稱的第一個標籤讀出地址：IPv4 只檢查形狀（`^[0-9]+-[0-9]+-[0-9]+-[0-9]+$`，不檢查八位元組 ≤ 255），其餘一律把橫線換成冒號當作 IPv6，之後原樣放進 `pfctl -t "$2" -T replace $want`（`$want` 沒加引號，SC2086 被關掉），`EMPTY` 也沒檢查，pfctl 的錯誤訊息被丟棄。一個設定錯誤或被竄改的 DNS 回答會讓 pfctl 拿到任意字串。
4. **Nagios 範例沒有跳脫輸出。** `examples/nagios/check_emfor.sh` 的 jq 程式把後端的 `reason` 與服務名稱放進外掛輸出：文字裡的 `|` 會被 Nagios 當作 perfdata 分隔符，名稱裡的 `'` 會弄壞 perfdata 的引號標籤。reason 來自檢查（exec 檢查的 reason 是 stdout 第一行，已跳脫控制字元，但 `|` 與 `'` 仍在）。

## What Changes

1. `watch.rs` 的測試改為輪詢等 `failure()` 出現（有期限），不再假設通知與失敗同時發生。只動測試，不動 `SecretWatch` 的行為。
2. `fuzz.yml` 的 `Fuzz` 步驟把輸入放進 `env:`（`FUZZ_SECONDS`），`run:` 裡以 `"$FUZZ_SECONDS"` 引用，並在 shell 裡檢查它只有數字。
3. pf 腳本：每個從 SRV 目標名稱讀出的地址要通過驗證才放進集合（IPv4：四個八位元組各 0–255；IPv6：八組各 1–4 個十六進位字元）；通不過的略過並在 stderr 說一行；`EMPTY` 與資料表名稱在啟動時驗證；`pfctl` 失敗時錯誤訊息的第一行放進 `die` 的訊息；`$want` 的分詞改為明確的迴圈或在驗證後保留 `set -f`，不再依賴 shellcheck 例外。
4. Nagios 腳本：外掛輸出文字裡的 `|` 換成 `/`，換行與回車換成空白；perfdata 標籤裡的 `'` 雙寫（`''`，Nagios 的規則）。
5. 兩個範例各加測試：pf 的驗證用一個假的 `drill` 與假的 `pfctl`（放在 `PATH` 前面）在 Docker 裡測，不需要 FreeBSD；Nagios 的在既有的 `tests/nagios/inside.sh` 加一個 reason 含 `|` 的情境。CHANGELOG 一個條目。

## Non-Goals (optional)

- 不做 `env_clear`（exec 檢查與 plugin 的環境變數白名單），那是相容性決定，另外討論。
- 不釘 Dockerfile 的基底映像 digest（30 個 `uses:` 已全部是完整 SHA；`rust:1-*` 等基底映像的釘選是維護成本的取捨）。
- 「FreeBSD spike 鎖」查無此事（`tests/spike/run.sh` 用 `mktemp` 與 `trap`），不處理；`tests/resolver/freebsd.sh` 的固定 `/tmp/emfor.toml.good` 跑在拋棄式 VM 裡，不處理。
- 不在 FreeBSD 上跑 pf 的實機測試（`tests/pf/run.sh`）；那是發版前才做的手動測試。
- 不改 `fuzz.yml` 的 `${{ matrix.* }}`：它們來自檔案本身，不是可被外部控制的輸入。

## Impact

- Affected specs: none
- Affected code:
  - Modified: `crates/emfor-daemon/src/peers/watch.rs`（只有 `tests` 模組）、`.github/workflows/fuzz.yml`、`examples/pf/emfor-pf-table.sh`、`examples/nagios/check_emfor.sh`、`tests/nagios/inside.sh`、`CHANGELOG.md`、可能的 `docs/manual/07-dns-recipes.md` 與 `docs/manual/08-operations.md`（只在它們描述了被改的行為時）
  - New: `tests/pf/validate.sh`
  - Removed: (none)
- Compatibility: no capability-level observable behavior changes；兩個範例腳本對正常輸入的輸出不變，只有不合法的輸入（地址、`|`、`'`）的處理不同。
