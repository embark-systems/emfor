## Why

設定可以拆成主檔加 `conf.d` 的多個片段（`emfor-conf-d`）之後，管理者要回答的問題變成「這個服務最後到底是什麼設定、每個值是哪個檔案設的」。現在只能自己讀每個檔案、在腦中依檔名順序合併。`emfor check-config` 只說合不合法，加上 `include_dir` 時列出讀了哪些檔案，不告訴你合併後的內容。

`emfor-conf-d` 當時把「印出合併後的有效設定與每個鍵的來源」列為非目標。`/spectra-discuss`（2026-11-16）與使用者定下這次的範圍：合併後的設定加上每個鍵的來源，不是套用預設值之後的完整設定。

## What Changes

- 新旗標 `emfor check-config --show`：照 `check-config` 一樣讀檔、合併、驗證，錯誤與結束碼 2 完全相同；設定有效時，把合併後的設定以 TOML 印到標準輸出，並在每個值後面用註解寫明最後設定它的檔案。
- 標準輸出只有那份 TOML，所以可以 `emfor check-config --show > merged.toml`，再拿它跑 `emfor check-config --config merged.toml`（去掉註解後的內容與原本合併結果相同）；平常 `check-config` 印的 `config ok: ...` 與 `read <path>` 行在 `--show` 時改寫到標準錯誤。
- 沒有設定 `include_dir` 時，所有值都來自主檔，輸出只在開頭加一行註解說明合併自哪個檔案，不在每個值後面加註解。
- 不套用內建預設值：沒寫的鍵不會出現（`interval = "2s"` 這種預設值不顯示）。
- 不遮蔽任何值：設定只接受密碼檔的路徑（`password_file`、`secret_file`），不接受內嵌的密碼；plugin 的 `command` 陣列照原樣印出，與設定檔本身一樣。手冊提醒不要把祕密寫在 `command` 裡。
- 不加 `--show` 時，`check-config` 的輸出與現在逐位元組相同。
- 文件：手冊第 3 章（檢查設定檔、把設定拆到目錄）與第 8 章（指令表）、`docs/deploy/daemon.md`、CHANGELOG 的 Unreleased。
- 隨 0.5.0 之後的下一版發佈，這次不打 tag。

## Non-Goals

- 不印出套用預設值之後、每個服務實際生效的完整設定（那需要追蹤「這個值是預設值」與另一個序列化器）。
- 不顯示被設定檔「刪掉」的東西：合併規則本來就不能刪。
- 不遮蔽值，也不為 `command` 內可能出現的祕密做偵測。
- 不改 `emfor status` 或 daemon 的任何行為。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `service-config`: 新增 `check-config --show` 的要求（輸出內容、來源註解、標準輸出與標準錯誤的分工、無效設定時不輸出、不加旗標時輸出不變）。

## Impact

- 程式：`crates/emfor-daemon/src/cli.rs`（`Command::CheckConfig` 的旗標與輸出）、`crates/emfor-daemon/src/config.rs`（把合併後的 table 與 `Layers::origins` 轉成帶註解的 TOML）。
- Spec：`docs/spectra/specs/service-config/spec.md`。
- 文件：`docs/manual/` 與 `docs/manual.zh_TW/` 的第 3、8 章、`docs/deploy/daemon.md`、`CHANGELOG.md`。
- 相容性：新增選用旗標，屬 `docs/compatibility.md` 允許的相容變更；`--show` 的輸出格式不承諾穩定，只承諾「去掉註解後是合法的 TOML、內容等於合併後的設定」。
