# 19. 詞彙表

[手冊](README.md) · 上一章：[18. 設定參考](18-config-reference.md)

每個詞條先列英文，括號裡是這份手冊用的中文；手冊裡直接用英文的詞，標為「沿用英文」。

| 詞彙 | 意義 |
| --- | --- |
| **affinity key**（沿用英文） | 應用程式傳給 `lookup` 的字串；同一個字串，`sticky` 會給同一個 backend。 |
| **all down**（全部 down） | 服務沒有任何 backend 在層級 1–4 裡（沒有一個健康）。這時查詢回什麼，由 `on_all_down` 決定。 |
| **assigned checkers**（負責主機） | 聯防群組裡，用投票決定某個 backend 狀態的三個成員。用雜湊選出，每個成員算出來的都一樣。 |
| **backend**（沿用英文） | 提供某個服務的一個 `ip:port`。 |
| **backup**（備援） | 只有在沒有任何主要 backend 通過檢查時才會用的 backend。 |
| **balance**（沿用英文） | 從候選裡挑一個的方式：`round-robin`、`sticky`、`failover`、`weighted`、`locality`、`fastest`。 |
| **candidates**（候選） | 查詢可以挑的 backend，也就是第一個不是空的層級。 |
| **check**（檢查） | 對一個 backend 做一次測試，由 `tcp`、`http`、`exec` 或 plugin 來做。 |
| **check definition digest**（檢查定義 digest） | 服務的 `check`、`check_options`、`timeout` 與 plugin 指令算出的雜湊，用來在成員之間比對。 |
| **client library**（client 函式庫） | `libemfor` 和它的包裝函式庫；讀 store，回傳一個 backend。 |
| **disagreement**（意見不一致） | 成員投票判定 down，但這台主機自己檢查是 up 的 backend。 |
| **drain**（沿用英文） | `drain = true`：這個 backend 永遠不會被選到，但還是會被檢查。 |
| **`emfor-check`**（沿用英文） | emfor 附帶的 plugin 程式，提供資料庫和各種網路協定的檢查。 |
| **`EMP1`、`EMP2`、`EMP3`**（沿用英文） | 聯防訊息的格式，依序是：簽署；用 ChaCha20-Poly1305 加密；用 XChaCha20-Poly1305 加密。 |
| **`error`（result）**（`error` 結果） | 因為這台主機本身的問題而沒能完成的檢查。不算 backend 失敗。 |
| **fall**（沿用英文） | 連續幾次 `down` 結果，backend 就會變成 `unhealthy`。 |
| **fast pool**（pool） | `fastest` 用的：檢查延遲接近最低值的那幾個 backend。 |
| **fast recheck run**（快速重查） | 有成員回報某個 backend down 時，開始每 200 ms 檢查一次（最多 `fall` 次）。 |
| **flapping**（反覆變化） | 一個 backend 在十分鐘內狀態變了五次以上。 |
| **heartbeat**（沿用英文） | daemon 每秒寫進 store 的時間；client 靠它判斷 store 是不是新的。 |
| **history**（狀態歷史） | 每個 backend 最近 20 次的狀態變化，存在 store 裡，重新啟動也不會消失。 |
| **interval**（沿用英文） | 同一個 backend 兩次檢查之間的間隔。 |
| **jitter**（沿用英文） | 加在每個 `interval` 上的隨機時間，讓檢查不會全部擠在同一刻。 |
| **live (member)**（存活成員） | 最近 3 秒內有收到它送來的有效訊息的成員。 |
| **lock file**（lock 檔） | `<store>-lock`，也就是 LMDB 的 reader 表；每個 reader 都要能寫它。 |
| **lookup**（查詢） | 透過 client 函式庫、`emfor lookup` 或 DNS，問某個服務該用哪個 backend。 |
| **on_all_down**（沿用英文） | 服務全部 down 時的處理方式：`open`（照樣回答）或 `closed`（以 `ALL_DOWN` 失敗）。 |
| **peer group**（聯防群組） | 同一個 site 裡互相分享故障通知、一起對 backend 投票的 daemon。 |
| **plugin**（沿用英文） | 長時間執行的程式，daemon 把檢查請求送給它，彼此用協定 1 溝通。 |
| **reason**（沿用英文） | 說明檢查為什麼失敗的文字。 |
| **reload**（沿用英文） | 收到 `SIGHUP` 時重新讀取設定，不用重新啟動。 |
| **rise**（沿用英文） | 連續幾次 `up` 結果，backend 就會變成 `healthy`。 |
| **service**（服務） | 一個名稱，加上一份 backend 清單、一種檢查和一種 balance 方式。 |
| **site**（沿用英文） | 資料中心的標籤，主機和 backend 都可以設。 |
| **slow start**（慢啟動） | 恢復的 backend 分到的份額，在 `slow_start` 期間分十個階段逐步增加。 |
| **SRV record**（SRV 記錄） | 帶有 port、優先序和權重的 DNS 記錄；emfor 每個候選回一筆。 |
| **stale (store)**（過期的 store） | heartbeat 比 `EMFOR_MAX_STALENESS`（3 s）還舊，或是 0 的 store：這時每個 backend 都當作不健康。 |
| **state**（狀態） | `unknown`、`healthy` 或 `unhealthy`。 |
| **store**（沿用英文） | daemon 寫入、client 讀取的 LMDB 檔案，預設是 `/var/lib/emfor/state.mdb`。 |
| **tier**（層級） | 查詢依序嘗試的幾組 backend 之一，從「最近一次檢查通過的健康主要 backend」一路到「所有 backend」。 |
| **zone**（沿用英文） | emfor 負責回答的 DNS 網域，預設是 `svc.emfor`。 |
