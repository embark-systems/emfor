# 19. 詞彙表

[手冊](README.md) · 上一章：[18. 設定參考](18-config-reference.md)

每個詞條先列英文，括號內是這份手冊使用的中文；手冊直接沿用英文的詞，標為「沿用英文」。

| 詞彙 | 意義 |
| --- | --- |
| **affinity key**（沿用英文） | 應用程式傳給 `lookup` 的字串，讓 `sticky` 對它給出同一個 backend。 |
| **all down**（全部 down） | 服務沒有任何 backend 位於層級 1–4（沒有一個是健康的）。由 `on_all_down` 決定查詢回傳什麼。 |
| **assigned checkers**（負責主機） | 在聯防群組中，以投票決定某個 backend 狀態的三個成員。以雜湊選出，每個成員算出的都一樣。 |
| **backend**（沿用英文） | 提供某個服務的一個 `ip:port`。 |
| **backup**（備援） | 只在沒有任何主要 backend 通過檢查時才使用的 backend。 |
| **balance**（沿用英文） | 如何從候選中選出一個：`round-robin`、`sticky`、`failover`、`weighted`、`locality`、`fastest`。 |
| **candidates**（候選） | 查詢可以從中挑選的 backend：第一個不是空的層級。 |
| **check**（檢查） | 對一個 backend 的一次測試，由 `tcp`、`http`、`exec` 或 plugin 進行。 |
| **check definition digest**（檢查定義 digest） | 服務的 `check`、`check_options`、`timeout` 與 plugin 指令的雜湊，在成員之間比對。 |
| **client library**（client 函式庫） | `libemfor` 與它的包裝函式庫；讀取 store 並回傳一個 backend。 |
| **disagreement**（意見不一致） | 依成員投票是 down、但這台主機自己的檢查是 up 的 backend。 |
| **drain**（沿用英文） | `drain = true`：這個 backend 絕不會被選到，但仍會被檢查。 |
| **`emfor-check`**（沿用英文） | 隨 emfor 附上的 plugin 程式，提供資料庫與網路協定的檢查。 |
| **`EMP1`、`EMP2`、`EMP3`**（沿用英文） | 聯防訊息的格式：簽署；以 ChaCha20-Poly1305 加密；以 XChaCha20-Poly1305 加密。 |
| **`error`（result）**（`error` 結果） | 因為這台主機上的問題而無法進行的檢查。它不算是 backend 的失敗。 |
| **fall**（沿用英文） | 連續幾個 `down` 結果會讓 backend 變成 `unhealthy`。 |
| **fast pool**（pool） | 用於 `fastest`：檢查延遲接近最低值的那些 backend。 |
| **fast recheck run**（快速重查） | 某個成員回報 backend down 時開始的每 200 ms 一次的檢查（最多 `fall` 次）。 |
| **flapping**（反覆變化） | 一個 backend 在十分鐘內狀態變化五次以上。 |
| **heartbeat**（沿用英文） | daemon 每秒寫進 store 的時間；client 用它判斷 store 是否新鮮。 |
| **history**（狀態歷史） | 每個 backend 最近 20 次狀態變化，保存在 store 中，重新啟動後仍保留。 |
| **interval**（沿用英文） | 一個 backend 兩次檢查之間的時間。 |
| **jitter**（沿用英文） | 加在每個 `interval` 上的隨機部分，讓檢查不會同時執行。 |
| **live (member)**（存活成員） | 最近 3 秒內收到過它有效訊息的成員。 |
| **lock file**（lock 檔） | `<store>-lock`，LMDB 的 reader 表；每個 reader 都需要寫入權限。 |
| **lookup**（查詢） | 透過 client 函式庫、`emfor lookup` 或 DNS，詢問某個服務該用哪個 backend。 |
| **on_all_down**（沿用英文） | 服務全部 down 時，`open`（照樣回答）或 `closed`（以 `ALL_DOWN` 失敗）。 |
| **peer group**（聯防群組） | 同一個 site 的 daemon，互相分享故障通知，並對 backend 投票。 |
| **plugin**（沿用英文） | daemon 送檢查請求給它的長時間執行程式，使用協定 1。 |
| **reason**（沿用英文） | 說明檢查為什麼失敗的文字。 |
| **reload**（沿用英文） | 收到 `SIGHUP` 時重新讀取設定，不需要重新啟動。 |
| **rise**（沿用英文） | 連續幾個 `up` 結果會讓 backend 變成 `healthy`。 |
| **service**（服務） | 一個名稱，帶有 backend 清單、一個檢查與一種 balance 方式。 |
| **site**（沿用英文） | 資料中心的標籤，用於主機與 backend。 |
| **slow start**（慢啟動） | 恢復的 backend 分到的份額在 `slow_start` 期間分十個階段增加。 |
| **SRV record**（SRV 記錄） | 帶有 port、優先序與權重的 DNS 記錄；emfor 為每個候選回答一筆。 |
| **stale (store)**（過期的 store） | heartbeat 比 `EMFOR_MAX_STALENESS`（3 s）舊、或為 0 的 store：每個 backend 都被當作不健康。 |
| **state**（狀態） | `unknown`、`healthy` 或 `unhealthy`。 |
| **store**（沿用英文） | daemon 寫入、client 讀取的 LMDB 檔案，預設為 `/var/lib/emfor/state.mdb`。 |
| **tier**（層級） | 查詢依序嘗試的 backend 集合之一，從「最近一次檢查通過的健康主要 backend」到「每個 backend」。 |
| **zone**（沿用英文） | emfor 回答的 DNS 網域，預設為 `svc.emfor`。 |
