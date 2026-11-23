## Problem

daemon 啟動後馬上退出時，`emfor status` 可能印出 `store format version (missing); this emfor reads versions 1 and 2` 並以 exit 2 結束。store 的格式並沒有問題：它是空的，因為 daemon 還沒來得及寫入。使用者會把這行當成版本不合，去查錯方向。實際發生在 FreeBSD：舊的 process 已占著 DNS port，新 daemon 開好 store、綁不上 port 就退出。

## Root Cause

daemon 先建立並開啟 store，之後才綁 DNS；`meta/schema_version` 要等 writer 第一次發布才寫入。daemon 在兩者之間退出，就會留下一個沒有任何 key 的 store。`crates/emfor-daemon/src/status.rs` 的 `Reader::read` 把「沒有 `meta/schema_version`」和其他讀不懂的版本一視同仁，印出 `(missing)`。

## Proposed Solution

`Reader::read` 在 `meta/schema_version` 不存在、且 store 裡沒有 `meta/heartbeat` 與任何 `svc/` 開頭的 key 時，改回報 `store is empty: the daemon has not written to it yet; if it should be running, check its log`，exit 2 不變。`meta/schema_version` 不存在、但有 heartbeat 或服務記錄時，才是真正的格式問題，維持現有 `(missing)` 訊息。手冊第 16 章加一列疑難排解，把這個訊息對應到「daemon 啟動後馬上退出」（例如 DNS port 被占用）。

## Non-Goals

- 不改 daemon 的啟動順序（先開 store、後綁 DNS）。
- 不改 `emfor status` 的其他訊息。

## Success Criteria

- `status.rs` 的單元測試：空 store 得到 empty 訊息；有 heartbeat 沒版本得到 `store format version (missing); ...`；版本 3 得到版本 3 訊息。
- `tools/docker-test.sh` 通過。
- 手冊第 16 章（en、zh_TW）與 `CHANGELOG.md` 已更新；`tools/check-manual-links.py` 回報 0 problems。

## Impact

- Affected specs: status-command
- Affected code:
  - Modified: crates/emfor-daemon/src/status.rs, docs/manual/16-troubleshooting.md, docs/manual.zh_TW/16-troubleshooting.md, CHANGELOG.md
