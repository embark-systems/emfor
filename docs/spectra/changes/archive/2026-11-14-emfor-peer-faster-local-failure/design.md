## Context

聯防群組（spec `peer-group`）把檢查分工：每台 backend 有三台負責主機每個 `interval` 檢查；其他成員在兩台負責主機意見一致時照投票結果走，`split()`（`crates/emfor-daemon/src/schedule.rs`）規定連續 `OWN_CHECK_EVERY - 1` = 4 次投票決定後，第 5 次一定自己檢查。自己的檢查若得到 `down`，而負責主機仍回報 `up`，結合後的結果是 `local: <reason>`，之後 `last_own_up` 為 false，所以每個 `interval` 都自己檢查，直到 `fall` 次失敗才標成 `unhealthy`。

快速重查（每 200 ms，最多 `fall` 次）目前只在收到其他成員的 `down` 通知時啟動：backend 檢查迴圈等待下一次檢查時，若 `peers.recheck` 收到通知且 `starts_fast_run(...)` 成立，就把 `fast_left` 設為 `fall` 並立刻檢查。快速重查進行中，`split()` 的 `in_fast_run` 讓每次都自己檢查；`fast_left` 在每次檢查後遞減，結果為 `up` 或 backend 變成 `unhealthy` 時歸零，歸零時記下 `last_run_end`。

只有非負責主機看得到的故障（這台主機到 backend 的網路問題），依上述排程推算（預設 `interval` 2 秒、`fall` 3，故障剛好發生在一次自己檢查之後）：第 2、4、6、8 秒由投票決定，第 10 秒自己查到第一次失敗，第 12、14 秒再失敗，約 14 秒才標成 `unhealthy`；沒有聯防群組時約 6 秒。這是推算值，任務 3.1 的測試以暫停的時鐘確認新排程的實際檢查時間點。

## Goals / Non-Goals

**Goals:**

- 只有非負責主機看得到的故障，在那台主機上最慢約 6.4 秒（預設值）標成 `unhealthy`：最慢第 6 秒自己查到失敗，再以快速重查在約 400 ms 內完成另外 2 次失敗。
- 沿用現有的快速重查機制與啟動條件，不新增設定。

**Non-Goals:**

- 負責主機、沒有 `[peers]`、`peers = false` 的服務、沒有負責主機（少於三台存活成員）時的排程不變。
- `error` 結果不啟動快速重查；票數不足（`u` < 2）或區域判定為 down（`d` ≥ 2）時也不啟動，因為那時本來就每次檢查或由投票決定。
- 不改 `rise` / `fall` 的計算、reason 文字、`down` 通知與 `result` 訊息的送出規則、peer 訊息格式與 store 格式。
- 不讓自己檢查的頻率可調整。

## Decisions

### 非負責主機每 3 次排定檢查至少自己檢查一次

`OWN_CHECK_EVERY` 從 5 改成 3：連續 2 次投票決定後，下一次必須自己檢查。這把「最多要等多久才自己查到故障」從約 5 個 `interval` 縮短到約 3 個。代價是 N 台成員時每台 backend 每個 `interval` 約 3 + (N − 3) / 3 次檢查，原本約 3 + (N − 3) / 5 次（10 台成員約 5.3 次，原本 4.4 次）。只改頻率時最慢約 10 秒（推算），所以要和下一個決定一起做。

### 本機失敗、區域為 up 時啟動快速重查

非負責主機自己的檢查得到 `down`，而其他負責主機的新鮮票中 `up` ≥ 2 且 `down` < 2（結合後的結果是 `local: <reason>`）時，套用結果後若 `starts_fast_run` 成立（backend 仍為 `healthy`、沒有進行中的快速重查、上一輪結束已超過 `fall × interval`），就開始一輪快速重查，和收到 `down` 通知時相同。剩下的 `fall − 1` 次檢查在約 `2 × 200 ms`（`fall` 3 時）內完成，取代每個 `interval` 一次。

條件限定在「其他負責主機回報 up」：這正是因分工而晚發現的情況。`u` < 2 時本來就每次自己檢查，行為等同沒有聯防群組；`d` ≥ 2 時區域判定已決定結果。負責主機自己的失敗不啟動：它每個 `interval` 都檢查，延遲和沒有聯防群組時相同。

啟動條件沿用 `starts_fast_run`，所以一輪快速重查結束後 `fall × interval` 內不會再啟動，抖動不會造成連續的快速重查。

### 由 quorum 的 Combined 告訴排程這是「區域為 up 時的本機失敗」

`Quorum::combine` 已經在檢查後重新數一次新鮮票並決定 `local:`。在 `Combined` 加一個布林欄位（例如 `local_failure`），在「這台不是負責主機、自己的結果是 `down`、其他負責主機的新鮮票 `up` ≥ 2 且 `down` < 2」時為 true；`schedule.rs` 的 `combine()` 把它和結果一起回傳給 backend 檢查迴圈。這樣用的是檢查完成當下的票，和 `local:` reason 的判斷一致，而不是檢查前 `split()` 看到的票。

不考慮的做法：
- 用 reason 是否以 `local: ` 開頭來判斷：`u` = 1 時也會是 `local:`，而且靠字串判斷行為不穩固。
- 只改頻率或只加快速重查：推算最慢分別約 10 秒與 10.4 秒，都還明顯慢於沒有聯防群組的 6 秒。

## Implementation Contract

**行為：**
- 非負責主機，其他負責主機都回報 `up`、自己的檢查也是 `up` 時，`interval = 1s`、`jitter = 0%` 下 10 次排定檢查中只在第 1、4、7、10 次自己檢查；`emfor_peer_checks_skipped_total` 增加 6。
- 同上，第 4 次自己檢查得到 `down`（`fall = 3`）：之後 200 ms、400 ms 各自己檢查一次，第二次後 backend 為 `unhealthy`，reason `local: <reason>`；同時記錄一行 `info`：`backend <backend> of <service> fails here while its assigned checkers see it up (<reason>); checking now`；變成 `unhealthy` 的那次是自己的檢查，所以照原規則送出一則 `down` 通知。
- 負責主機自己的 `down` 不啟動快速重查（下一次檢查在 1 個 `interval` 後）。
- `error` 結果、`u` < 2、`d` ≥ 2、backend 套用後已非 `healthy`、快速重查進行中、上一輪結束不到 `fall × interval` 時都不啟動。

**介面：**
- `crates/emfor-daemon/src/schedule.rs`：`OWN_CHECK_EVERY` = 3；`combine()` 回傳結果與「是否為區域 up 時的本機失敗」；backend 檢查迴圈在套用結果、更新 `fast_left` 之後判斷是否開始快速重查（`fast_left = fall`，下一次等待用 `fast_interval`）。
- `crates/emfor-daemon/src/quorum.rs`：`Combined` 新增布林欄位，`Quorum::combine` 填入。

**驗收：**
- 單元測試（`schedule.rs`）：`split` 的表格測試改成「連續 2 次後自己檢查」；`sparse_own_checks_while_the_region_is_up`、`local_failure_while_the_region_is_up`、`region_down_without_an_own_check` 依新的 spec scenario 更新；新增負責主機不啟動快速重查的測試；新增或擴充 quorum 測試涵蓋 spec 例子「whether a failed own check starts a fast recheck run」每一列。
- 每個新行為以 RED 檢查：暫時改回或拿掉後，對應測試失敗。
- 整合測試 `crates/emfor-daemon/tests/peers.rs` 的 `split_checks` 在新頻率下仍通過（spare 的自己檢查約為負責主機的三分之一）。
- 依 RELEASING.md 在 macOS 與 Debian 容器跑完整檢查。

**範圍：**
- 範圍內：`schedule.rs`、`quorum.rs` 的上述改動與測試；手冊第 1、3、8 章（英文與中文）、`docs/deploy/daemon.md`、`docs/spectra/LANGUAGE.md`「投票決定」、`CHANGELOG.md`。
- 範圍外：peer 訊息、store、client、設定選項、DNS。

## Risks / Trade-offs

- [檢查量增加] → 非負責主機的檢查從每 5 次 1 次變成每 3 次 1 次；10 台成員時每台 backend 每個 `interval` 約多 0.9 次檢查（推算）。文件寫出新的公式。
- [對抖動更敏感] → 在「區域為 up 時的本機失敗」這種情況，約 0.6 秒內連續 `fall` 次失敗就會標成 down，和收到 `down` 通知時的快速重查一樣；`starts_fast_run` 的冷卻時間（`fall × interval`）避免連續觸發。
- [整合測試時間敏感] → `split_checks` 以實際時間計算檢查次數，頻率變高後 spare 與負責主機的比例變小；若斷言不再成立，改成依新頻率的比例並在任務中記錄實測次數。
