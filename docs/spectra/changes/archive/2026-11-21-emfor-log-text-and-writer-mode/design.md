## Context

見 proposal。三項互相獨立，只是同一輪審查的低嚴重度項目。

## Goals / Non-Goals

**Goals:** plugin 的文字不能用 CR 或 bidi 字元改寫日誌；密碼不因伺服器回顯進失敗原因；`.writer` 不被其他使用者鎖住。**Non-Goals:** 見 proposal。

## Decisions

以下是我（助手）做的設計選擇，使用者可以改。

### 跳脫沿用既有規則

`normalize_reason` 已經定義了「控制字元與隱形格式字元用 `escape_default` 跳脫」。把這一步抽成 `escape_unprintable`，`normalize_reason` 先呼叫它再加 512 字元上限；plugin 日誌用它但不加上限（stderr 行已先由 `cut` 限制 1000 字元；跳脫在截斷之後，所以一行最多約 1000 個字元各展開成至多 10 個字元，截斷不會切在跳脫序列中間）。這樣只有一份「什麼要跳脫」的定義。

### redis 用字串取代遮蔽密碼

伺服器的錯誤文字裡凡出現密碼（非空）就換成 `***`；使用者名稱不遮蔽（不是機密）。只遮蔽完整的密碼字串，不處理伺服器只回顯一部分的情況。回顯的是 `describe` 與 `auth: {e}` 兩處，都走同一個函式。

### `.writer` 的模式

建立時 `OpenOptionsExt::mode(0o600)`；開啟後若模式不同，`set_permissions(0o600)`，失敗記 `warn` 並繼續（檔案可能屬於另一個使用者，例如以 root 跑過之後改用 emfor 使用者；此時不能因為 chmod 失敗就讓 daemon 起不來）。flock 不受影響。

## Implementation Contract

- **行為**：見上。
- **介面**：`checker::escape_unprintable(&str) -> String`（crate 內部）；`redis.rs` 內的 `redact`。
- **失敗模式**：chmod 失敗只警告。
- **驗收**：`tools/docker-test.sh` 通過；新測試：stderr 行含 `\r` 與 U+202E 時日誌裡是跳脫後的文字，Hello 的名稱與版本同樣；redis 的錯誤文字含密碼時失敗原因不含密碼；新建與既有 0644 的 `.writer` 都是 0600。
- **範圍內**：上述三項、手冊 01 的說明（英文與 zh_TW）、CHANGELOG。**範圍外**：見 proposal 的 Non-Goals。

## Risks / Trade-offs

- 只遮蔽完整密碼：伺服器若回顯密碼的一部分，仍會漏。這是已知限制。
- 日誌裡的 plugin 文字若本來就含 `\r`，現在顯示成 `\r` 字樣；這是想要的。

## Open Questions

- 無。
