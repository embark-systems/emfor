## Problem

peer-group spec 的要求「Peer secret file owner」寫 `secret_file` 與 `previous_secret_file` 的擁有者須是 root 或「the effective user the daemon runs as」（daemon 執行時的有效使用者），並說這項檢查在啟動、`emfor check-config` 與 reload 都適用。但 `emfor check-config` 是另一個行程，由執行它的人啟動；它比對的是那個人，不是 daemon 的使用者。照 spec 字面讀，以 `alice` 身分執行 `check-config` 檢查一個 `emfor` 使用者擁有的 secret 會被拒絕，spec 卻說這個檔案合格（因為 daemon 以 `emfor` 執行）。spec 與行為對不上。

## Root Cause

要求的措辭沒有把「做檢查的行程」與「daemon」區分開。`read_secret`（`crates/emfor-daemon/src/config.rs`）用 `nix::unistd::geteuid()` 取做這次檢查的行程自己的有效使用者，再交給 `owner_trusted(uid, euid)`；daemon 啟動與 reload 時這個行程就是 daemon，`check-config` 時是執行該指令的人。程式與 CHANGELOG、手冊都是這個行為，只有 spec 的措辭錯。

## Proposed Solution

只改 peer-group spec 的措辭，不改程式，行為不變：

- 要求改成「擁有者須是 root（uid 0），或做這次檢查的行程的有效使用者」，並註明啟動與 reload 時做檢查的是 daemon，`emfor check-config` 時是執行它的使用者。錯誤訊息的文字（「owned by uid N, not root or the user running emfor」）不變。要求同時寫明擁有者檢查在檔案開啟之後才做（`read_secret` 先 `open`、取 metadata、確認是一般檔案，才比對擁有者），所以讀不到檔案的帳號得到的是 `cannot read` 錯誤，不是擁有者錯誤；擁有者檢查本身不看群組，但群組仍可能決定檔案讀不讀得到。
- 範例表的欄位從「Daemon user」改為「Checking user」（做檢查的使用者）；擁有者與使用者的數值與結果不變，只有兩個錯誤列的 Result 欄文字改成「error naming uid N」，因為實際訊息結尾還有 `(uid N)`，表格不再引用完整訊息。
- 新增兩個情境：非 root 的帳號執行 `emfor check-config`（且讀得到檔案，例如在檔案的群組內），擁有者是 daemon 使用者的 secret 被拒絕，擁有者是 root 的被接受。既有的兩個情境（「Secret owned by another user」「Secret owned by root or by the checking user」）的 WHEN 也補上「讀得到檔案」的前提。

這直接改掉根因（spec 沒區分做檢查的行程），不是治標。

## Non-Goals

- 不改 `read_secret`、`owner_trusted` 或任何程式與測試。
- 不改錯誤訊息文字。
- 不改 CHANGELOG 與手冊：CHANGELOG 的 Unreleased 條目已寫「`emfor check-config` compares the owner with the user that runs it」，手冊的 `secret_file` 列寫「root 或執行 emfor 的使用者」，與行為一致。
- 不改權限檢查、一般檔案與 FIFO 檢查或 secret 長度。

## Success Criteria

- `docs/spectra/specs/peer-group/spec.md` 的「Peer secret file owner」不再把檢查者寫成 daemon，並有一個情境說明 `emfor check-config` 由非 root 帳號執行時的結果。
- 該要求的每個情境與範例表都能由 `crates/emfor-daemon/src/config.rs` 既有行為讀出，沒有新行為；測試 `peer_secret_owner_is_root_or_the_daemon_user` 的五列對應範例表的五列。
- `spectra validate emfor-secret-owner-wording` 通過，`spectra analyze emfor-secret-owner-wording` 沒有 Critical 或 Warning。

## Impact

- Affected specs: `peer-group`（修改「Peer secret file owner」）
- Affected code:
  - Modified: `docs/spectra/specs/peer-group/spec.md`（經由 delta spec）
  - New: 無
  - Removed: 無
