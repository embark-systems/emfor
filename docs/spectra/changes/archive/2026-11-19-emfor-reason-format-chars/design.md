## Context

`normalize_reason` 在 `crates/emfor-daemon/src/checker/mod.rs`，對每個 check 結果的 reason 做清理：tab、CR、LF 跳脫成 `\t`、`\r`、`\n`；其他 Cc 控制字元跳脫成 `\u{..}`；超過 512 個字元截斷。判斷「要不要跳脫」原本只有 `char::is_control()`，只涵蓋 Cc。reason 來自不可信的後端、plugin 與腳本，會顯示在 `emfor status`、log 與狀態歷史。

## Goals / Non-Goals

**Goals:**

- 讓不可見、會隱藏文字或改變文字順序的格式字元，在 reason 裡也跳脫成可見的 `\u{..}`。
- 規格、程式碼、手冊三者一致。

**Non-Goals:**

- 不跳脫整個 Unicode Cf 類別。
- 不新增 crate 依賴。
- 不改截斷長度或其他字元的處理。

## Decisions

### 用明列的字元範圍，不查 Unicode 一般類別

Rust 標準庫沒有一般類別（general category）的查詢，`char::is_control()` 只是 Cc。要涵蓋整個 Cf 需要新依賴（例如 `unicode-general-category`）或內嵌一份幾百個碼位的表。選擇：在 `is_format_control` 明列會隱藏或改變文字順序的範圍。

替代方案：整個 Cf。好處是定義簡單；代價是新依賴（上一個 commit 才為了維護狀態移除 `humantime`）或一張要跟 Unicode 版本更新的表，而 Cf 的其餘字元（阿拉伯數字符號、樂譜符號等）既不隱藏也不改變順序，跳脫它們沒有安全上的好處。

### 副作用：emoji 的 ZWJ 也會被跳脫

U+200D（零寬連接符）在 emoji 序列與某些文字裡有正當用途，但 reason 是診斷訊息，多半是 ASCII；跳脫它只影響顯示。不為它開例外，因為 ZWJ 同樣能被用來藏標記。

## Implementation Contract

- **Behavior**：reason 中的 U+00AD、U+061C、U+180E、U+200B–U+200F、U+202A–U+202E、U+2060–U+2064、U+2066–U+206F、U+FEFF、U+FFF9–U+FFFB、U+E0000–U+E007F 存成 `\u{十六進位小寫}`，其餘字元的處理不變。
- **Interface**：`checker::normalize_reason(&str) -> String` 簽章不變；新增模組內私有函式 `is_format_control(char) -> bool`。
- **Failure modes**：無；函式純粹、不會失敗。
- **Acceptance criteria**：`cargo test -p emfor-daemon --lib checker::tests::reasons_are_normalized` 通過，涵蓋規格範例表中的每一列；`spectra validate emfor-reason-format-chars` 通過；archive 後 `docs/spectra/specs/health-checking/spec.md` 的該需求含新範圍與範例。
- **Scope boundaries**：範圍內是 `normalize_reason`、規格、手冊第 4 章（英文與繁體中文）與 CHANGELOG；範圍外是 `peer-group` 規格（`read_secret` 的改動不需要規格變更）、store 格式、其他 checker 的行為。

## Risks / Trade-offs

- 明列的範圍會落後於 Unicode 新增的隱形字元；之後出現新的就要手動加進去（`is_format_control` 與規格同步）。
- 還沒用 macOS 或 FreeBSD 手動跑過；邏輯是純字串處理，與平台無關。
