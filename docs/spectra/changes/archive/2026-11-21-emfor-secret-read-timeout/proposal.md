## Problem

peer 任務每秒讀一次 `secret_file`。如果那個檔案放在卡住的網路檔案系統上，讀取不會回來，整個 peer 迴圈跟著停住：不送 heartbeat、不做存活檢查、不處理收到的訊息。其他成員在 3 秒內就會把這台主機看成不存活，而這台主機自己的 daemon、檢查與 DNS 都還在正常運作，所以從外面看不出原因。這是 TODO 58 的觀察項目，也是 TODO 67 的一項。

## Root Cause

`crates/emfor-daemon/src/peers/task.rs` 的 `run` 迴圈在 `tick` 分支（每秒一次）先 `refresh_secret(...).await`，再做 `check_liveness` 與送 heartbeat；`settings.changed()` 分支在 `secret_file` 換路徑時也 `await` 同一個函式。`refresh_secret` 把 `read_secret` 丟進 `spawn_blocking` 後直接等它，沒有逾時，所以一個不回來的讀取會讓迴圈永遠停在那一行。光加逾時不夠：被卡住的 blocking 執行緒無法取消，每次逾時後再開一個新讀取，卡住的執行緒會一直累積，塞滿 blocking 執行緒池。

## Proposed Solution

迴圈不再等讀取。新增一個小結構 `SecretReader`，同一時間最多保有一個進行中的讀取，把讀取函式當作欄位（測試可以換成會卡住的函式）。這個方案直接解決根本原因：迴圈不再等待讀取（不再停住），而且同時最多一個讀取（不再累積執行緒）；它不是只壓住症狀的緩解。

- **tick 或換路徑時沒有進行中的讀取**：啟動一個，最多等它 200 毫秒（設計選擇，非量測值）。健康的檔案系統上讀取在這個時間內結束，新的 secret 和現在一樣在同一個 tick 套用，所以「每輪 heartbeat 前檢查 secret_file」的規則維持原意。沒結束就留在背景，迴圈用最後一個有效的 secret 繼續做 `check_liveness` 與送 heartbeat。
- **tick 時已有進行中的讀取**：不等、不再開新的。它已結束就套用結果，然後照上一條啟動新的；還沒結束而且已經跑了超過 5 秒（命名常數，設計選擇），就用既有的 `WARN_EVERY` 節奏（每分鐘最多一次）記一行 `warn`，說明讀取還沒結束、保留最後有效的 secret。
- 晚到的結果和任何結果一樣套用。換路徑時舊讀取還在進行，就等它結束後再讀新路徑（每次啟動讀取時都用當下的路徑）。

- **變更偵測改用檔案監看（需求變更，在使用者決定後加入）**：能監看時（Linux 的 inotify、FreeBSD 與 macOS 的 kqueue）監看 `secret_file` 所在的目錄（kqueue 另外監看檔案本身），收到事件才讀檔，變更不必等下一個 tick 才被看到；另外每 30 秒保險讀一次，避免事件漏掉。建不起來監看、或平台沒有時，退回每個 tick 讀一次檔（現狀），並記一行 `info`。讀取仍走上面不阻塞迴圈的做法。

## Non-Goals

- 不改設定載入時（啟動、重載驗證）對 `secret_file` 的讀取：那裡卡住只會拖慢那次啟動或重載，不會讓執行中的群組停擺。
- 不改 plugin、DNS 任務，RADIUS 的 `password_file`，也不處理 exec 子程序的 `env_clear`（TODO 67 的另一項）。
- 不保證 NFS 等不產生本機事件的檔案系統上的即時性（使用者決定不考慮 NFS）；那種情況靠 30 秒保險讀取最終一致。
- 不嘗試取消被卡住的 blocking 執行緒（做不到）；也不新增設定項目讓管理者調整 200 毫秒與 5 秒。

## Success Criteria

- 能監看的平台上，`secret_file` 被改寫或被換檔（寫新檔再 rename 蓋掉）後，新 secret 在下一輪 heartbeat 前生效（讀取在 200 毫秒內完成時），不必等到下一個 tick 的輪詢；Linux 上以測試驗證，kqueue 在實機驗證前標為未驗證。
- 監看建立失敗時退回每個 tick 讀檔，行為與現在相同。

- 讀取函式卡住時，peer 迴圈仍每秒送出 heartbeat，被測的另一個成員仍看到這台主機存活，而讀取函式整段時間只被呼叫一次。
- 卡住超過 5 秒後出現一行 `warn`，一分鐘內不會出現第二行；讀取回來後 secret 被套用。
- 正常的讀取在同一個 tick 套用新 secret（保留現有行為）。
- 既有的 `apply_secret` 與 secret 輪替測試全部照舊通過；`tools/docker-test.sh` 在 Debian 容器通過。

## Impact

- Affected specs: `peer-group`（requirement "Peer secret reload"）
- Affected code:
  - Modified: `crates/emfor-daemon/src/peers/task.rs`（`refresh_secret` 與 `run` 迴圈，`start_on` 多一個內部入口讓測試注入讀取函式）、`crates/emfor-daemon/Cargo.toml`（`nix` 加 `inotify`、`event` 與 `poll` features（`poll` 給 Linux 的 inotify 等待用））
  - New: `crates/emfor-daemon/src/peers/watch.rs`（檔案監看）
  - Removed: 無
- 文件：`docs/manual/03-configuration.md` 與 `docs/manual.zh_TW/03-configuration.md`（"The secret file is read again every second" 那段）、`CHANGELOG.md` 的 Unreleased。
