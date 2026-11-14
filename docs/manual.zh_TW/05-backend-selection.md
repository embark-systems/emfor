# 5. 選擇 backend

[手冊](README.md) · 上一章：[4. 健康檢查](04-health-checks.md) · 下一章：[6. Client 函式庫](06-clients.md)

應用程式查詢一個服務時，client 函式庫先找出**候選**（通常是最近一次檢查通過的健康 backend，依設定檔順序），再由服務的 `balance` 挑出一個。DNS server 回答 A 與 AAAA 查詢時也一樣，差異列在本章最後。

## balance 方式

| `balance` | client 查詢 | 適用於 |
| --- | --- | --- |
| `round-robin`（預設） | 輪流使用候選 | 無狀態服務 |
| `sticky` | 同一個 affinity key 在 backend 健康期間都拿到同一個 backend | 快取與 session，同一位使用者應該一直打到同一個 backend |
| `failover` | 永遠是設定檔順序中的第一個候選 | 一個 primary 加上 standby |
| `weighted` | 依每個 backend 的 `weight` 比例 | 規格不同的 backend |
| `locality` | 優先使用主機所在 `site` 的候選，輪流 | 多個資料中心 |
| `fastest` | 在檢查回應最快的 backend 之間輪流 | 網路距離不同的 backend |

### round-robin

每個行程為每個服務保留一個計數器，起始值隨機，所以同一主機上的各行程不需協調也會分散開來。每次查詢取下一個候選。

### sticky

應用程式傳入一個 **affinity key**，例如使用者或 session ID。client 為每個候選計算 `MD5(key + backend id)`，取值最大的候選（rendezvous hashing）。因此同一個 key 在每個行程、每台主機、每種語言中都選到同一個 backend；該 backend 故障時只有它的 key 會移走，恢復後再回來。沒有 key（或 key 為空）時，`sticky` 的行為同 `round-robin`。

`weight` 為 w 的 backend 分到的 key 是權重 1 的 w 倍：它的值是 `MD5(key + id)` 與 i 從 1 到 w − 1 的 `MD5(key + id + "#" + i)` 中最大的一個，所以一台權重 3 和兩台權重 1 的 backend 放在一起時，權重 3 那台分到 3/5 的 key。權重全為 1 時，選擇結果和這個版本以前相同。調高某台的權重只會把 key 移到它身上，調低只會把 key 從它身上移走。慢啟動不改變帶 key 的 `sticky` 所用的權重。store 裡超過 100 的權重（daemon 不會寫出）當成 100。每次查詢要為候選的權重總和各算一次 MD5：10 台權重 1 約 0.8 µs，10 台權重 100 約 72 µs（在 Apple M4 Max 上量測），所以查詢量大的 sticky 服務請讓權重保持小。

### failover

設定檔順序中第一個健康的 backend。把 primary 列在最前面。搭配資料庫 plugin 的 `role` 選項時，只有目前的 primary 會通過檢查，所以新的 primary 到達 `rise` 後，`failover` 就會跟著切換。

### weighted

```toml
backends = ["10.0.4.11:9200", { addr = "10.0.4.12:9200", weight = 3 }, "10.0.4.13:9200"]
```

在一個行程中，每 5 次查詢裡 `10.0.4.12` 會被回傳 3 次，其他各 1 次。只計算健康的 backend；有 backend 故障時，權重會重新加總。

### locality

```toml
site = "dc1"

[services."cache.svc.emfor"]
check = "tcp"
balance = "locality"
backends = [
  { addr = "10.0.5.11:6379", site = "dc1" },
  { addr = "10.0.5.12:6379", site = "dc1" },
  { addr = "10.1.5.11:6379", site = "dc2" },
]
```

只要主機所在 site 中有任何 backend 是候選，查詢就在這些 backend 之間輪流；否則在所有候選之間輪流。沒有設定最上層的 `site` 時，`locality` 的行為同 `round-robin`。

### fastest

daemon 依平滑後的檢查延遲，決定哪些健康的 backend 算快，把這個 **pool** 寫進 store；client 在 pool 中輪流。backend 的延遲在最低值的 1.5 倍以內（或比最低值多不超過 1 ms）時加入 pool，超過最低值的 2 倍（且多出 2 ms 以上）時離開，避免小幅變動讓 backend 反覆進出。pool 為空或 store 過期時，`fastest` 的行為同 `round-robin`。

### 不認得的值

client 讀到不認得的 `balance`（由較新的 daemon 寫入）時，使用 `round-robin`。不認得的 backend `state` 視為 `unhealthy`，所以只有在 `open` 的服務沒有更好的選擇時才會用到這個 backend；不認得的 `on_all_down` 視為預設的 `open`。`emfor status` 把它們顯示為 `unrecognized` 與 `on_all_down=unknown`。store 中不認得的欄位會被忽略。

## 檢查開始失敗的 backend

backend 要連續失敗 `fall` 次才會轉為不健康；在那之前它仍是健康的，但最近一次檢查失敗，`reason` 有值（`emfor status` 會顯示）。只要還有至少一個最近一次檢查通過的健康 backend，這種正在失敗的 backend 就不列入候選，所以查詢與 DNS 回答在它第一次檢查失敗時就不再選它。全部健康的 backend 都在失敗時，它們全部都是候選，和以前一樣。`down` 與 `error` 結果同樣算數：如果是 checker 本身出問題、所有 backend 一起失敗，結果不會改變。

- client 看到新的 `reason` 最多晚約一秒（daemon 隨 heartbeat 把 `reason` 的變動寫入 store）；DNS server 從 daemon 的記憶體回答，立即生效。
- 使用 `sticky` 時，正在失敗的 backend 的 key 會改對到其他候選，在它下一次檢查通過後回來。
- 在 peer group 中，即使 peer 仍判定 backend 為 up，本機自己的檢查失敗也算數（`reason` 以 `local: ` 開頭），所以本機會立即不再選它。

## 備援 backend

`backup = true` 的 backend 保留給其他 backend 都不能用的時候，例如另一個資料中心的副本或規格較小的機器。store 新鮮時，候選依下列順序，取第一個不是空的集合：

1. 最近一次檢查通過的主要 backend（不是備援的）；
2. 最近一次檢查通過的備援；
3. 健康的主要 backend，正在失敗的也算；
4. 健康的備援，正在失敗的也算。

所以只要沒有主要 backend 通過檢查，備援就接手，不必等主要 backend 轉為不健康；主要 backend 下一次通過時，備援就讓回去。在選出的集合中，`balance` 照常運作：`failover` 取設定檔順序的第一台備援，`weighted` 用備援的權重，依此類推。`fastest` 在主要 backend 與備援之間各自判斷哪些夠快，所以很快的備援不會把主要 backend 擠出 pool。完全沒有健康的 backend 時由 `on_all_down` 決定，主要與備援一視同仁（見下文）。過期的 store 也用全部 backend。

DNS 回答依同樣的順序；SRV 記錄只列出選出的集合。使用備援需要能讀 store 格式版本 2 的 client（[第 6 章](06-clients.md#store-格式版本)）。

## 慢啟動（slow start）

剛恢復的 backend 可能快取還是冷的、連線池還沒建立。設定 `slow_start`（例如 `slow_start = "30s"`）後，從 `unhealthy` 轉為 `healthy` 的 backend 會在這段時間內逐步增加分到的流量，而不是立刻拿到全部份額。

- 這段時間平均分成 10 級。第一個十分之一的時間裡，backend 以它權重的 1/10 計算，第二個十分之一以 2/10，依此類推；`slow_start` 過後回到全額。權重在 `weighted` 是 backend 的 `weight`，其他方式都是 1。
- 只要有至少一個候選正在慢啟動，`round-robin`、`weighted`、`locality`、`fastest` 與沒有 key 的 `sticky` 就依這些降低後的權重挑選。`locality` 與 `fastest` 照常先縮小到同 site 或 pool。`failover` 與有 key 的 `sticky` 不受影響。
- 只有「恢復」才算：daemon 啟動時為 `unknown` 的 backend，以及 reload 新增或重新開始檢查的 backend，立刻拿到全部份額。再次轉為不健康的 backend 會結束慢啟動。
- 只有部分 backend 恢復、其他 backend 已經健康時才有效果。只有一台 backend 的服務照樣把全部流量給它；同時恢復的 backend 彼此份額相同。
- 過期的 store 沒有慢啟動；時鐘往回調時視為慢啟動結束。
- 早於這個功能的 client 會忽略它：恢復的 backend 立刻拿到全部份額。

DNS 的 A/AAAA 回答依同樣降低後的權重挑選，SRV 記錄以它作為 weight（[第 7 章](07-dns.md#srv-記錄)）。

## 沒有任何 backend 健康時

| `on_all_down` | 查詢 | DNS A/AAAA/SRV |
| --- | --- | --- |
| `open`（預設） | 在下列 open 的候選之間挑選 | 以下列 open 的候選回答 |
| `closed` | 以 `ALL_DOWN` 失敗 | `SERVFAIL` |

`open` 的候選依下列順序，取第一個不是空的集合：

1. 最近一次檢查通過（沒有 `reason`）的 `unknown` backend；
2. 全部 `unknown` backend；
3. 全部 backend。

`unhealthy` 的 backend 已經連續失敗 `fall` 次；`unknown` 的還沒有定論（剛啟動、新增或重設，或檢查只回錯誤超過 `fall × interval`），所以比較有機會成功。daemon 剛啟動時全部都是 `unknown`，全部都是候選。過期的 store 用全部 backend。早於這條規則的 client 會在全部 backend 之間挑選。

查詢結果不會告訴應用程式這是來自全部 down 的服務。當「試一個 backend」比失敗好時（可能是檢查本身出錯）用 `open`；當把流量送到壞掉的 backend 比快速失敗更糟時用 `closed`。

## 過期的 store

daemon 的 heartbeat 超過 `EMFOR_MAX_STALENESS`（預設 3 秒），或 daemon 已正常停止時，client 不把任何 backend 當作健康，由 `on_all_down` 決定，與上面完全相同。`fastest` 服務會忽略它的 pool。查詢永遠不會卡住或等待 daemon。

## DNS 的差異

DNS server 依據相同的健康狀態、從 daemon 的記憶體回答，但：

- A 查詢只考慮 IPv4 backend，AAAA 查詢只考慮 IPv6 backend。服務沒有該位址族的 backend 時回答為空。client 查詢則把兩種位址族一起考慮。
- round-robin 與 weighted 的選擇由 daemon 進行，每個查詢一次，而不是每個應用程式行程各自計算。
- DNS 沒有 affinity key，所以 `sticky` 的行為同 `round-robin`。
- SRV 回答列出所有候選，priority 與 weight 依 `balance` 設定（[第 7 章](07-dns.md#srv-記錄)）。
- resolver 的快取可能在 TTL 期間一直回傳同一個答案。
