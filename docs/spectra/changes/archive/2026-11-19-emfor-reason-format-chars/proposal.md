## Problem

check 結果的 reason 在存進 store、顯示在 `emfor status` 與寫進 log 之前，會經過 `normalize_reason`。規格「Reasons are bounded and printable」只要求跳脫 Unicode 類別 Cc 的控制字元，並寫明「其他字元一律保持原樣」。後端、plugin 或腳本因此可以在 reason 裡放不可見的格式字元：雙向控制字元（例如 U+202E 從右到左覆寫）、零寬字元（U+200B、U+FEFF 等）、軟連字號、tag 字元。這些字元不會印出任何東西，或者讓前後的文字反向顯示，所以 `emfor status` 或 log 裡的一行可以看起來像是另一回事：例如一段含 U+202E 的 reason，在終端機裡會把它後面的文字反向顯示，或在看得見的文字中藏進看不見的標記。

這是 2026-11-19 的 CASA 自我評估（docs/TODO.md 第 67 項的第 5 點）列出的缺口：reason 來自不可信的後端，清理只涵蓋 Cc。

## Root Cause

`normalize_reason`（`crates/emfor-daemon/src/checker/mod.rs`）用 `char::is_control()` 決定要不要跳脫，而它只涵蓋 Cc。Rust 標準庫沒有 Unicode 一般類別的查詢，所以 Cf（格式字元）一直沒有被處理；規格當時也照這個行為寫成「其他字元保持原樣」。

## Proposed Solution

除了 Cc，下列「會隱藏文字或改變文字順序的不可見格式字元」也跳脫成 `\u{十六進位}`（小寫、無前導零、大括號，與 Cc 相同的形式）：

- U+00AD（軟連字號）、U+061C、U+180E
- U+200B 到 U+200F、U+202A 到 U+202E、U+2060 到 U+2064、U+2066 到 U+206F
- U+FEFF
- U+FFF9 到 U+FFFB
- U+E0000 到 U+E007F（tag 字元）

這一組不是整個 Cf 類別：Cf 的其餘字元（例如 U+0600 阿拉伯數字符號）既不隱藏也不改變文字順序，維持原樣；一般的非 ASCII 文字（`é`、`日本`、`ñ`、`℃`）也維持原樣。不加新的 crate 依賴。

實作已經在工作樹裡（`is_format_control` 與 `normalize_reason`，測試在 `reasons_are_normalized`），手冊第 4 章（英文與繁體中文）和 CHANGELOG 的 Unreleased 也已描述。這個 change 補上規格，讓規格、程式碼與文件一致。

另一項同一天做的改動不需要改規格：peer 的 `secret_file` 改成先開檔再檢查打開的那個檔案（`read_secret`）。`peer-group` 規格對 `secret_file` 的要求只有絕對路徑、讀得到、至少 32 bytes、other 不可讀、group 與 other 不可寫，以及它們的錯誤訊息；這些訊息都沒變。新增的「不是一般檔案」錯誤（例如指向 FIFO）是規格沒有談到的情況，不與任何要求抵觸。

## Non-Goals

- 不跳脫整個 Cf 類別，也不新增依賴來查 Unicode 類別。
- 不改長度上限（512 個字元）、tab／CR／LF 的跳脫，或其他字元保持原樣的規則。
- 不改 `peer-group` 規格。

## Success Criteria

- 規格「Reasons are bounded and printable」列出被跳脫的格式字元，並附範例：`up<U+202E>nwod` 存成 `up\u{202e}nwod`，`a<U+200B>b` 存成 `a\u{200b}b`，`<U+FEFF>x` 存成 `\u{feff}x`，`a<U+E0041>b` 存成 `a\u{e0041}b`。
- 規格範例也寫明 `é ok 日本` 與 `<U+0600>1 ñ ℃` 保持原樣。
- `cargo test -p emfor-daemon --lib checker::tests::reasons_are_normalized` 通過，且涵蓋上述每個範例。
- `spectra validate emfor-reason-format-chars` 通過。

## Impact

- Affected code:
  - Modified: `crates/emfor-daemon/src/checker/mod.rs`（已改，尚未 commit）
  - Modified: `docs/spectra/specs/health-checking/spec.md`（封存時套用 delta）
  - Modified: `docs/manual/04-health-checks.md`、`docs/manual.zh_TW/04-health-checks.md`、`CHANGELOG.md`（已改，尚未 commit）
  - New: 無
  - Removed: 無
