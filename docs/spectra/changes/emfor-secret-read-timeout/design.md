## Context

`crates/emfor-daemon/src/peers/task.rs` 的 `run` 迴圈每秒（`HEARTBEAT_EVERY`）在 `tick` 分支呼叫 `refresh_secret(...).await`，之後才做 `group.check_liveness` 與送 heartbeat；`secret_file` 換路徑的 `settings.changed()` 分支也 `await` 同一個函式。`refresh_secret` 把 `read_secret` 丟進 `spawn_blocking` 後直接等待，沒有逾時。讀取不回來時（例如卡住的 NFS），整個迴圈停住，其他成員 3 秒內就把這台主機視為不存活。

`apply_secret(group, read, warned, now)` 負責套用一次讀取的結果（有效且變更就輪替；讀不到或無效就保留最後有效的 secret，最多每 `WARN_EVERY` 警告一次）。本變更不改它的行為。

## Goals / Non-Goals

**Goals:**

- 一個不回來的 `read_secret` 不得延遲 heartbeat、存活檢查與收到訊息的處理。
- 同時最多一個讀取在進行，不論卡住多久（卡住的 blocking 執行緒無法取消，不能每秒再開一個）。
- 健康的檔案系統上維持現狀：新的 secret 在同一個 tick 套用。
- 卡住夠久時有一行可被管理者看到的警告。

**Non-Goals:**

- 設定載入時（啟動、重載驗證）對 `secret_file` 與 `previous_secret_file` 的讀取：那裡卡住只拖慢那一次啟動或重載。
- plugin、DNS 任務，RADIUS 的 `password_file`，exec 子程序的 `env_clear`。
- 取消被卡住的執行緒；新增設定項目調整下面的時間常數。
- NFS 等不產生本機事件的檔案系統上的即時性（使用者決定不考慮）。

## Decisions

以下是我（助手）做的設計選擇，使用者可以改。

### 迴圈不等讀取，狀態放在迴圈裡的 SecretReader

新增小結構 `SecretReader`，欄位為進行中的讀取 `Option<(JoinHandle<Result<Vec<u8>, String>>, Instant)>`（控制代碼與開始時間）與讀取函式 `fn(&Path) -> Result<Vec<u8>, String>`。正式程式碼用 `read_secret`；測試用內部入口注入會卡住的函式。取代 `refresh_secret` 的 `await`。選擇理由：只留在迴圈裡的狀態，不需要鎖，也不新增任務。

### 一個 tick 最多等 200 毫秒

沒有進行中的讀取時，啟動一個並最多等 `SECRET_READ_WAIT` = 200 毫秒（設計選擇，未量測）。健康的檔案系統上讀一個小檔遠低於這個值，所以新 secret 在同一個 tick、送 heartbeat 前套用，現有規則「每輪 heartbeat 前檢查 `secret_file`」維持原意。沒有在 200 毫秒內結束，讀取留在背景，迴圈用最後有效的 secret 繼續。確認值的方法：在 Debian 容器內的既有輪替測試必須維持通過（它們在正常檔案系統上依賴同一 tick 套用）；一個 tick 的最壞延遲上限就是 200 毫秒（相對 heartbeat 間隔 1 秒與存活判定 3 秒）。

### 用 nix 的 inotify 與 kqueue 監看，不加新套件

`crates/emfor-daemon` 已依賴 `nix`（features `fs`、`signal`、`process`、`user`），再加 `inotify`（Linux）、`poll`（Linux，inotify 執行緒的逾時等待）與 `event`（FreeBSD、macOS，kqueue）。不用 `notify` crate：它會多帶一串相依、要過 cargo-deny，也較難控制 macOS 的後端。新模組 `peers/watch.rs` 提供 `SecretWatch`：建立時對路徑開監看，成功回傳監看物件，失敗回傳錯誤（呼叫端退回輪詢）。

### 監看放在專用執行緒，以 Notify 通知迴圈

`SecretWatch` 的執行緒阻塞在 inotify 的 read 或 kqueue 的 kevent（逾時 1 秒，順便檢查停止旗標），收到事件就 `notify_one()` 一個 `tokio::sync::Notify`；迴圈多一個 select 分支等這個 Notify。理由：兩個平台用同一種結構；不依賴 kqueue 檔案描述元能否放進 `AsyncFd`（macOS 上未驗證）。`SecretWatch` 被丟棄時設停止旗標，執行緒在下一個逾時內結束。

### 監看什麼

- inotify：`secret_file` 所在的目錄，事件 `CLOSE_WRITE`、`MOVED_TO`、`CREATE`、`DELETE`、`ATTRIB`、`MOVE_SELF`/`DELETE_SELF`、佇列溢位（`Q_OVERFLOW`）都當作「可能變了」。以目錄為對象，是因為用寫新檔再 rename 蓋掉的方式換 secret 時，對檔案 inode 的監看會失效。
- kqueue：目錄的 kqueue 事件只涵蓋新增、刪除、改名，抓不到檔案內容的原地修改，所以同時監看目錄與檔案本身（`NOTE_WRITE`、`NOTE_EXTEND`、`NOTE_ATTRIB`、`NOTE_DELETE`、`NOTE_RENAME`、`NOTE_LINK`）；檔案被換掉後，事件之後重新打開檔案並重新註冊。
- 事件只表示「可能變了」，不解讀事件內容：一律交給 `SecretReader` 讀檔並由 `apply_secret` 判斷內容有沒有變。同一時間多個事件合成一次讀取（Notify 本身合併）。

### 讀檔的時機

- 能監看時：收到事件就讀；另外每 30 秒（`SECRET_SAFETY_READ`，設計選擇）保險讀一次，涵蓋漏掉的事件（inotify 佇列溢位、kqueue 換檔的競爭）與不產生本機事件的檔案系統。
- 不能監看時（建立失敗、平台沒有、執行緒回報監看失效，例如目錄被移除）：每個 tick 讀一次，與現狀相同。第一次退回時記一行 `info`，說明原因。
- `secret_file` 換路徑：丟掉舊的監看、對新路徑建立新的；建立失敗就退回輪詢。
- 迴圈啟動時建立監看；建立在第一次讀檔之前，這樣建立與第一次讀取之間發生的變更不會漏掉（監看先建、再讀）。
- 建立監看（開目錄、`inotify_add_watch`）也在 blocking 執行緒上，迴圈最多等 `SECRET_READ_WAIT`（200 毫秒）。逾時就記一行 `info`，監看繼續在背景建立，建立好之前每個 tick 讀檔；建立好時下一個 tick 接手並多讀一次（涵蓋建立前發生的變更）。（review 發現路徑在不回應的網路檔案系統上時，同步建立會卡住迴圈。）
- 事件的讀取間隔：距上次讀取開始不到 `SECRET_EVENT_GAP`（100 毫秒）的事件不立刻讀，只記下「有變更待讀」，由下一個 tick 讀；事件到來時若上一次讀取還沒結束，同樣記下，結束後的下一個 tick 套用結果並再讀一次。因此事件最晚在下一次 heartbeat（1 秒內）被讀到；監看的是整個目錄，忙碌的目錄不會讓每個事件都讀一次檔。（review 發現的兩點。）

### 已有進行中讀取時不等也不再開新的

tick 時若上一次讀取還在進行：已經結束就套用結果並照上一段啟動新的；沒結束就什麼都不做，只檢查是否超過 `SECRET_READ_SLOW` = 5 秒（設計選擇）。超過就用既有的 `warned` 與 `WARN_EVERY` 節奏記一行 `warn`：`peers.secret_file: the read has not finished after N seconds; keeping the last valid secret`（N 為已經過的整數秒）。晚到的結果與任何結果一樣交給 `apply_secret`。

### 換路徑：舊讀取結束後才讀新路徑

`settings.changed()` 帶來新的 `secret_file` 時，若舊讀取還在進行，不啟動新的；每次啟動讀取都用迴圈目前持有的路徑，所以舊讀取結束後的下一次啟動就讀新路徑。結果是舊路徑讀取的結果（若晚到）仍會被套用一次，這與「同一時間最多一個讀取」的取捨一致，並寫進風險。

### 讀取函式 panic 或執行緒被取消

`JoinHandle` 回傳錯誤時，當作一次讀取失敗，走 `apply_secret` 的錯誤路徑，與現在 `unwrap_or_else` 的行為相同。

## Implementation Contract

- **行為**：`run` 迴圈在任何讀取狀態下，每個 tick 都送出 heartbeat 並做 `check_liveness`，也持續處理收到的訊息。讀取函式整段卡住期間只被呼叫一次。卡住超過 5 秒後一行 `warn`，之後每分鐘最多一行；讀取回來後 `apply_secret` 套用結果（變更且有效則輪替並記現有的 `info`）。
- **監看**：能監看時（Linux inotify、FreeBSD 與 macOS kqueue）收到事件才讀 `secret_file`，另外每 30 秒保險讀一次；不能監看時每個 tick 讀一次並記一行 `info`。換檔（寫新檔再 rename 蓋掉）與原地改寫都要被偵測到。
- **介面**：`SecretWatch`（`peers/watch.rs`，crate 內部）；`SecretReader`（`task.rs` 內部，非 pub）；`start_on` 維持既有簽章，內部委派給帶讀取函式參數的入口；常數 `SECRET_READ_WAIT`（200 毫秒）、`SECRET_READ_SLOW`（5 秒）與 `SECRET_SAFETY_READ`（30 秒）；`crates/emfor-daemon/Cargo.toml` 的 `nix` 加 features `inotify`、`event`。設定檔格式、指標、對外訊息格式都不變。
- **失敗模式**：讀取錯誤與 panic 照舊保留最後有效 secret；警告用既有的每分鐘一次節奏，不新增 metric。
- **驗收**：`tools/docker-test.sh` 內 `cargo test` 通過，包含新測試（注入卡住的函式：heartbeat 持續、函式只被呼叫一次、5 秒後一行警告且一分鐘內不重複、讀取回來後 secret 套用）與既有 `apply_secret` 及 secret 輪替測試，以及 inotify 測試（原地改寫與 rename 換檔都在下一輪 heartbeat 前生效、建立失敗時退回輪詢）；kqueue 在 macOS 與 FreeBSD 實機手動驗證前標為未驗證；`cargo clippy -D warnings` 與 `cargo fmt --check` 通過。
- **範圍內**：`peers/task.rs` 的讀取流程、`peers/watch.rs`、對應測試、手冊 ch.3 的一句話（中英）、CHANGELOG Unreleased。**範圍外**：見 Non-Goals。

## Risks / Trade-offs

- kqueue 路徑（FreeBSD、macOS）只能在實機驗證；Debian 容器只測得到 inotify。驗證前 spec 的 kqueue 情境標為未驗證。
- 事件漏掉時，secret 最久 30 秒才被看到（保險讀取），遠小於輪替窗口 60 秒；NFS 上其他主機改檔沒有本機事件，也靠這個上限。
- 多一條執行緒，常駐阻塞在事件上；監看建立失敗的唯一後果是退回輪詢。

- 讀取卡住期間 secret 輪替不會生效：這台主機繼續用舊 secret 簽章與驗證，在輪替窗口（簽 60 秒、收 120 秒）之內若檔案一直卡住，其他主機的新 secret 訊息會被丟為 `auth`。這是檔案系統卡住的後果，本變更只保證群組不因此停擺並在 5 秒後警告。
- 每個 tick 最壞多 200 毫秒延遲（輪詢模式的 tick 與事件觸發的讀取都適用）（只在讀取沒有在 200 毫秒內完成時）。200 毫秒、5 秒與 30 秒的取值是設計選擇，未以真實卡住的 NFS 量測。
- 被卡住的 blocking 執行緒在讀取回來之前一直佔用一個 blocking 池執行緒（只有一個）。
- 換路徑時舊路徑晚到的結果會被套用一次，之後新路徑才被讀。

## Open Questions

None
