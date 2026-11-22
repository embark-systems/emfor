## Problem

在對照程式碼寫 0.7.0 新手冊時，發現三處 spec 的文字和實際行為不符（程式碼是對的，也都有測試或可測的行為）：

1. **checker-plugins，「Plugin output line limit」**：spec 說超長的 stderr 行會「記錄前 65536 位元組」。實際上 daemon 對任何一行 stderr 最多只記錄前 1000 個字元；無效的 stdout 行與握手答覆在 log／錯誤訊息裡則切成前 200 個字元。spec 沒有這兩個切斷。
2. **checker-plugins，「Plugin check results」**：spec 說 `emfor-plugin` crate 送出「至多 512 個字元加 `…`」，且 daemon 的 reason 正規化「保留不超過這個長度」。實際上 crate 送出 512 字元加 `…`（共 513 個字元），daemon 再切成前 511 個字元加 `…`（共 512 個字元），與 health-checking spec 一致。spec 也沒有提到 crate 自己產生的兩個 reason：`invalid target ip "<ip>"` 與 `check was cancelled`。
3. **database-checks，「Check options validation」與「Passwords from password_file only」**：spec 只要求 `password_file` 去掉一個結尾換行後非空。實際上不是有效 UTF-8 的密碼也會被拒絕：驗證時 `password_file`: `not valid UTF-8`，檢查時 `error`，reason 為 `password_file: not valid UTF-8`；檢查時讀到空檔則是 `password_file: file is empty`，spec 也沒寫。

## Root Cause

這些行為在不同版本加進程式碼（或在審查後修正），當時只改了程式與測試，沒有回頭改 spec：stderr 行改為 1000 字元、crate 的 reason 上限、非 UTF-8 密碼的拒絕都是如此。spec 與程式碼之間沒有檢查，不一致一直留到手冊審查才被看見。

## Proposed Solution

只改 spec 文字，讓它說出程式碼已經在做的事；不改任何執行行為。

- checker-plugins：把 stderr 行的記錄長度改為 1000 個字元，補上無效 stdout 行與握手答覆的 200 字元切斷；reason 的長度改成 crate 513 個字元、daemon 保留 512 個字元，並寫出 crate 的兩個 reason。
- database-checks：`password_file` 要求「去掉一個結尾換行後非空，且是有效 UTF-8」，檢查時的對應 reason 也寫進去。
- 每一句修正過的陳述，都要有一個對得上測試的情境。已有測試的情境直接引用；沒有測試的（無效 stdout 行與握手答覆的 200 字元切斷、檢查時的非 UTF-8 與空檔密碼、`invalid target ip` 的 reason 文字），在 tasks.md 加上補測試的任務，測試補在既有的測試檔裡。`check was cancelled` 只在檢查工作被取消、而且不是 panic 時才會出現，現有程式沒有可以穩定觸發它的路徑，所以 spec 只寫出這個 reason，不為它另寫情境。

## Non-Goals

- 不改任何執行行為：切斷長度、reason 文字與驗證規則都維持現狀。
- 不改其他 spec、新手冊與 `docs/TODO.md`。
- 不處理 CHANGELOG 已發佈文字裡的連結。

## Success Criteria

- 兩份 delta spec 改完後，`spectra validate emfor-spec-code-alignment` 通過，`spectra analyze` 沒有 Critical 或 Warning。
- 修正過的每一句陳述，都有一個情境，且情境能對到一個通過的測試（既有或這次新增的）。
- 新增的測試在 Docker 裡通過（`tools/docker-test.sh`），既有測試不變：`crates/emfor-daemon/tests/plugins.rs` 的 `oversized_stderr_line_is_cut` 與 `crates/emfor-plugin` 的 `long_reasons_are_cut` 仍通過。
- 封存後 `docs/spectra/specs/checker-plugins/spec.md` 與 `docs/spectra/specs/database-checks/spec.md` 不再有與上面三項衝突的文字。

## Impact

- Affected specs: checker-plugins、database-checks
- Affected code:
  - Modified: crates/emfor-daemon/tests/plugins.rs、crates/emfor-daemon/tests/support/test_plugin.rs、crates/emfor-plugin/src/lib.rs（只改測試）、crates/emfor-check/src/options.rs（只改測試）
  - New: 無
  - Removed: 無
