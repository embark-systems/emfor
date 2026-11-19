## 1. 實作與測試（工作樹已完成，尚未 commit）

- [x] 1.1 依設計「用明列的字元範圍，不查 Unicode 一般類別」，`normalize_reason` 把 U+00AD、U+061C、U+180E、U+200B–U+200F、U+202A–U+202E、U+2060–U+2064、U+2066–U+206F、U+FEFF、U+FFF9–U+FFFB、U+E0000–U+E007F 存成 `\u{十六進位小寫}`，不加新依賴（`crates/emfor-daemon/src/checker/mod.rs` 的 `is_format_control`）。驗證：`cargo test -p emfor-daemon --lib checker::tests::reasons_are_normalized` 通過。
- [x] 1.2 符合規格「Reasons are bounded and printable」的範例表：`up<U+202E>nwod`、`a<U+2066>b<U+2069>`、`a<U+200B>b`、`<U+FEFF>x`、`a<U+E0041>b` 被跳脫，`é ok 日本` 與 `<U+0600>1 ñ ℃` 保持原樣。驗證：同一個測試 `reasons_are_normalized` 逐列斷言這些輸入與輸出。
- [x] 1.3 設計「副作用：emoji 的 ZWJ 也會被跳脫」：U+200D 與同範圍的字元一樣被跳脫，不開例外。驗證：`reasons_are_normalized` 含 `a<U+200D>b` 存成 `a\u{200d}b` 的斷言。

## 2. 文件（工作樹已完成，尚未 commit）

- [x] 2.1 手冊第 4 章（`docs/manual/04-health-checks.md`、`docs/manual.zh_TW/04-health-checks.md`）說明不可見格式字元也被跳脫；`CHANGELOG.md` 的 Unreleased 有一段說明並註明規格當時尚未更新。驗證：內容審閱，三處文字與規格的範圍一致。

## 3. 規格與封存

- [x] 3.1 `spectra validate emfor-reason-format-chars` 通過，且 `spectra analyze emfor-reason-format-chars` 沒有 Critical。驗證：兩個指令的輸出。
- [x] 3.2 封存後 `docs/spectra/specs/health-checking/spec.md` 的「Reasons are bounded and printable」含新字元範圍、新的 scenario 與擴充後的範例表；並更新 CHANGELOG 該段，移除「規格尚未更新」的說明。驗證：`spectra archive emfor-reason-format-chars --preview --json` 顯示的 delta 計畫，以及封存後檢視該需求的內容。
