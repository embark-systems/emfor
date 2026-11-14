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

client 讀到不認得的 `balance`（由較新的 daemon 寫入）時，使用 `round-robin`。store 中不認得的欄位會被忽略。

## 檢查開始失敗的 backend

backend 要連續失敗 `fall` 次才會轉為不健康；在那之前它仍是健康的，但最近一次檢查失敗，`reason` 有值（`emfor status` 會顯示）。只要還有至少一個最近一次檢查通過的健康 backend，這種正在失敗的 backend 就不列入候選，所以查詢與 DNS 回答在它第一次檢查失敗時就不再選它。全部健康的 backend 都在失敗時，它們全部都是候選，和以前一樣。`down` 與 `error` 結果同樣算數：如果是 checker 本身出問題、所有 backend 一起失敗，結果不會改變。

- client 看到新的 `reason` 最多晚約一秒（daemon 隨 heartbeat 把 `reason` 的變動寫入 store）；DNS server 從 daemon 的記憶體回答，立即生效。
- 使用 `sticky` 時，正在失敗的 backend 的 key 會改對到其他候選，在它下一次檢查通過後回來。
- 在 peer group 中，即使 peer 仍判定 backend 為 up，本機自己的檢查失敗也算數（`reason` 以 `local: ` 開頭），所以本機會立即不再選它。

## 沒有任何 backend 健康時

| `on_all_down` | 查詢 | DNS A/AAAA/SRV |
| --- | --- | --- |
| `open`（預設） | 在所有 backend 之間挑選，當作全部都健康 | 以所有 backend 回答 |
| `closed` | 以 `ALL_DOWN` 失敗 | `SERVFAIL` |

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
