# 7. DNS

[手冊](README.md) · 上一章：[6. Client 函式庫](06-clients.md) · 下一章：[8. 維運](08-operations.md)

無法使用 client 函式庫的程式（第三方執行檔、沒有 client 的語言、`CGO_ENABLED=0` 的 Go 建置）可以改用 DNS 解析服務名稱。emfor 的 DNS server 只回答自己的 zone，聽在 loopback 的一個 port 上；主機原本的 resolver 把這個 zone 轉送給它。

```
program --> host resolver (dnsmasq, unbound, systemd-resolved) --> other names: upstream DNS
                     |
                     +-- *.svc.emfor --> emfor on 127.0.0.1:5300
```

emfor 絕不是主機的一般 DNS server，所以 daemon 停止時，只有它 zone 裡的名稱無法解析。

## 啟用

```toml
[dns]
listen = ["127.0.0.1:5300"]   # default; not 5353, which is mDNS
zone = "svc.emfor"            # default
ttl = "1s"                    # default
rate_limit = 100              # default; UDP responses a second per client network
```

空的 `[dns]` table 使用預設值。server 同時聽 UDP 與 TCP。有位址無法 bind 時，daemon 以結束代碼 1 結束。TCP 同時最多服務 128 條連線（超過的會立即關閉），5 秒內沒有送出查詢的 TCP 連線會被關閉。daemon 執行中若有 listener 停止，daemon 會以結束代碼 70 結束，由 service manager 重新啟動。

只有名稱在 zone 之下的服務會被回答。啟動與 reload 時，daemon 會為每個在 zone 之外的服務記錄一則警告，也會為每個 backend 使用不同 port 的服務記錄一則警告（A 與 AAAA 回答不帶 port；這類服務請用 SRV 或 client 函式庫）。

### 速率限制

SRV 回答可以比查詢大上許多倍，所以其他主機連得到的 DNS server 可能被拿來用偽造來源的 UDP 查詢灌爆第三方。因此 server 會依 client 網段（IPv4 /24 或 IPv6 /56）計算 UDP 回應，每秒最多送出 `[dns] rate_limit` 個（預設 100）。超過之後，每兩個回應中有一個改成空的、設了 TC 旗標的回應：它比查詢還小，正常的 client 會改用 TCP 重試；其餘不回應。loopback 位址上的 client 與 TCP 一律不受限，所以預設的設定（`listen` 在 127.0.0.1，由本機 resolver 查詢）不受影響。`emfor_dns_rate_limited_total` 計算被限制的回應；如果站內轉送到 emfor 的 resolver 出現在這裡，就調高 `rate_limit`，或設為 `0` 關閉限制。

啟動時，daemon 會為每個不是 loopback 的 `listen` 位址記錄一則警告：任何連得到它的主機都能查詢每個服務。請用防火牆限制誰能連到這個 port。

設定 resolver 之前，先直接查詢確認：

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A          # Linux, macOS
drill -p 5300 api.svc.emfor @127.0.0.1           # FreeBSD
```

## 回答

| 查詢 | 回答 |
| --- | --- |
| 服務的 A / AAAA | 一筆記錄：依 `balance` 選出的該位址族 backend（[第 5 章](05-backend-selection.md#dns-的差異)），帶 `aa` flag。 |
| A / AAAA，沒有該位址族的 backend | `NOERROR`，沒有記錄（NODATA）。 |
| A / AAAA / SRV，沒有健康的 backend 且 `on_all_down = "closed"` | `SERVFAIL`。 |
| 服務或 `_<任意>._tcp.<服務>` 的 SRV | 每個候選一筆 SRV 記錄，位址放在 additional section。 |
| 服務的其他類型 | `NOERROR`，沒有記錄。 |
| zone 之下、但不是服務的名稱 | `NXDOMAIN`。 |
| zone 之外的名稱，或 class 不是 `IN` | `REFUSED`。 |
| EDNS 版本大於 0 | `BADVERS`。 |

TTL 是服務的 `dns_ttl`，否則為 `[dns] ttl`（預設 1 秒）。zone 有一筆合成的 SOA，其負向快取 TTL 也是 `[dns] ttl`。

## SRV 記錄

SRV 回答帶有每個 backend 的 port，並列出所有候選，選法與 A、AAAA 回答相同：最近一次檢查通過的主要 backend；沒有的話，最近一次檢查通過的[備援](05-backend-selection.md#備援-backend)；再沒有的話，全部健康的主要 backend 都在失敗時列出它們全部（見[檢查開始失敗的 backend](05-backend-selection.md#檢查開始失敗的-backend)），否則列出全部健康的備援。只要有健康的 backend，同一個回答就不會同時列出主要與備援；沒有任何健康且 `on_all_down` 為 `open` 時，列出 open 的候選（見[第 5 章](05-backend-selection.md#沒有任何-backend-健康時)）：先是 `unknown` 的 backend（最近一次檢查通過的優先），沒有才列出全部。IPv4 與 IPv6 backend 一起列出。Go 的 `net.LookupSRV("http", "tcp", "api.svc.emfor")`、gRPC 與類似的 client 會查詢 `_http._tcp.api.svc.emfor`，emfor 會回答。

| `balance` | Priority | Weight | 順序 |
| --- | --- | --- | --- |
| `failover` | 依設定檔順序 0、1、2… | 1 | 設定檔順序 |
| `weighted` | 0 | backend 的 `weight` | 設定檔順序 |
| `locality` | 在主機的 `site` 中為 0，其他為 1 | 1 | 每次查詢輪替 |
| `fastest` | 在 pool 中為 0，不在為 1 | 1 | 每次查詢輪替 |
| `round-robin`、`sticky` | 0 | 1 | 每次查詢輪替 |

有候選正在[慢啟動](05-backend-selection.md#慢啟動slow-start)時，除了 `failover` 以外的方式都把每台的 weight 設為降低後的權重：它的 `weight`（`weighted`）或 1，乘上 1 到 10 的級數。

每筆記錄的 target 是由 backend 位址衍生、放在服務名稱之下的名稱，不論 backend 是否健康，emfor 都會解析它：

- `10.0.0.11` → `10-0-0-11.api.svc.emfor`
- `2001:db8::11` → `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor`

透過 UDP 時，回應大小不超過 client 以 EDNS 宣告的大小（最多 1232 位元組），沒有 EDNS 時為 512 位元組。位址放不下時就省略位址；連 SRV 記錄都放不下時，回應會被截斷（TC），resolver 會改用 TCP 重試。名稱長度接近 255 位元組上限時，連單一 A 記錄或 SOA 都可能超過 512 位元組，這種回應同樣會被截斷。

## 從主機的 resolver 轉送

選擇你的主機所用的 resolver。以下片段使用預設值；如果改過 zone 或 port，請跟著調整。

**dnsmasq**（`/etc/dnsmasq.d/emfor.conf`，然後 `systemctl restart dnsmasq`）：

```
server=/svc.emfor/127.0.0.1#5300
```

**unbound**，也適用 FreeBSD 的 `local_unbound`（`/etc/unbound/unbound.conf.d/emfor.conf` 或 `/var/unbound/conf.d/emfor.conf`）：

```
server:
    do-not-query-localhost: no
    domain-insecure: "svc.emfor"

forward-zone:
    name: "svc.emfor"
    forward-addr: 127.0.0.1@5300
```

兩行 `server:` 設定都是必要的：unbound 預設不查詢 localhost，而會做驗證的 unbound 會拒絕這個未簽章的 zone。

**systemd-resolved**（`/etc/systemd/resolved.conf.d/emfor.conf`，需 systemd 246 以上，然後 `systemctl restart systemd-resolved`）：

```
[Resolve]
DNS=127.0.0.1:5300
Domains=~svc.emfor
```

**macOS**（僅供開發；檔案 `/etc/resolver/svc.emfor`）：

```
nameserver 127.0.0.1
port 5300
```

接著透過系統 resolver 測試：Linux 與 FreeBSD 用 `getent hosts api.svc.emfor`，macOS 用 `dscacheutil -q host -a name api.svc.emfor`。[docs/deploy/dns-resolver.md](../deploy/dns-resolver.md) 有經過驗證的設定方式與更多檢查方法。

## 搭配 nginx 使用

開源版的 nginx 沒有主動健康檢查（只有 `max_fails`），upstream 清單在 reload 之前也是固定的。nginx 1.27.3 或更新的版本可以改從 emfor 的 SRV 回答取得清單（`resolve` 與 `service=` 在那個版本之前只有商業版有），所以檢查失敗的 backend、你 drain 的 backend、你改的權重，都會在幾秒內傳到 nginx（先是 emfor 自己的檢查間隔，再加上 `valid` 的一秒），不需要 reload：

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;    # emfor 的 DNS server，每秒問一次

upstream api {
    zone api 64k;                              # 用 `resolve` 就一定要有
    server api.svc.emfor service=http resolve; # 查 _http._tcp.api.svc.emfor
}

server {
    listen 80;
    location / { proxy_pass http://api; }
}
```

`resolver` 直接指向 emfor（`[dns] listen`，預設 5300 port），不是主機的 resolver：nginx 有自己的 resolver 快取，`valid=1s` 讓它每秒重新問一次。`service=http` 只是名稱的前綴（`_http._tcp`）；emfor 會回答 `_<任意>._tcp.<服務名稱>`（[SRV 記錄](#srv-記錄)），所以 port 來自每一筆記錄。

nginx 怎麼處理這個回答，以下用 nginx 1.30.6（stable）與 1.31.8（mainline）測過：

- **健康狀態**：檢查失敗的 backend 不在回答裡，nginx 就不再送請求給它；它恢復後又回來。測試對每個變化最多等 10 秒；檢查間隔 200 毫秒時大約花 3 秒。
- **`drain = true`**（[第 5 章](05-backend-selection.md#drain-一個-backend)）：改 emfor 的設定並 reload emfor；不用動 nginx，那台 backend 不再收到新的請求。（nginx 已經對它開著的連線會怎樣，沒有試過。）
- **優先序**：nginx 把 priority 數字最小的記錄當 server，其他的當 `backup`。`balance = "failover"` 時，第一台健康的 backend 是唯一的 server，第二台是它的備援；`locality` 與 `fastest` 的 priority 0 與 1 也是同樣的用法。
- **`balance = "weighted"` 的權重**會被遵守：權重 3 與 1 時，80 個請求中有 60 個送到第一台。（nginx 的文件沒有說它會用 SRV 的權重。）
- **emfor 無法回答時**：服務沒有 backend 可以給（`on_all_down = "closed"` 且全部不健康，或每個 backend 都 drain），emfor 回 `SERVFAIL`；emfor 停掉也一樣。nginx 會記錄 `could not be resolved`，並**沿用它最後一次拿到的清單**，繼續送請求給那些 backend。對 nginx 的用戶端來說，要靠 nginx 自己的 `proxy_next_upstream` 與 `max_fails` 才會失敗。
- **HTTPS upstream**：記錄的目標是像 `10-0-0-11.api.svc.emfor` 的名稱。要把 `proxy_ssl_name`（並設 `proxy_ssl_server_name on`）設成 backend 憑證上的名稱。這一項沒有試過。

**較舊的 nginx。** 1.27.3 之前，`service=` 參數在 `nginx -t` 就被拒絕（`invalid parameter "service=http"`，用 1.26.3 看到的）。沒有它時，在 `proxy_pass` 用變數，讓 nginx 在執行時解析名稱，並讓服務的每個 backend 用同一個 port：

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;
server {
    listen 80;
    location / {
        set $backend api.svc.emfor;
        proxy_pass http://$backend:8080;
    }
}
```

A 回答只有一個 backend，由 `balance` 選出，所以 nginx 一秒內的所有請求都送給那一台（它把回答保留 `valid` 的時間）；輪替發生在秒與秒之間，不是請求與請求之間。測試中兩台 backend 都有被用到，平均程度沒有量。

[tests/nginx/](../../tests/nginx/run.sh) 在指定的 nginx 映像上執行這些檢查（`tests/nginx/run.sh nginx:stable`），包含較舊的 nginx 的情況。Integrations workflow 每週執行一次，emfor 的 DNS 或候選的程式有改動時也會執行。


在 FreeBSD 15.1（arm64，套件裡的 nginx 1.30.6）上，同樣的檢查全部通過，包含較舊的 nginx 的情況，食譜不需要改。nginx 套件讀的是 `/usr/local/etc/nginx/nginx.conf`，不是 `/etc/nginx`。Docker 版的測試本身不在那裡執行；腳本（`tests/nginx/inside.sh`）是手動跑的，需要 Linux 容器本來就有的三樣東西：`dig`（用 `drill` 包一層就行）、loopback 上的位址 `127.0.0.2`（`ifconfig lo0 alias 127.0.0.2/32`；FreeBSD 的 loopback 只有 `127.0.0.1`，較舊 nginx 那項檢查會把一個 backend 綁在 `127.0.0.2`），以及讓 `nginx` 讀 `/etc/nginx/nginx.conf`。

## 搭配 HAProxy 使用

HAProxy 可以用 SRV 回答來填一個 backend 的 server：`server-template` 寫上 SRV 查詢名稱與一個「槽」的數量，HAProxy 把記錄放進槽裡，回答改變時再重新查詢。把 emfor 的 DNS server 當作它的 resolver，檢查失敗的 backend、你 drain 的 backend、你新增的 backend，都會在幾秒內傳到 HAProxy，不需要 reload：

```
resolvers emfor
    nameserver emfor 127.0.0.1:5300
    accepted_payload_size 4096
    hold valid 1s
    resolve_retries 3
    timeout resolve 1s
    timeout retry 1s

backend api
    balance roundrobin
    server-template s 1-4 _http._tcp.api.svc.emfor resolvers emfor resolve-opts allow-dup-ip resolve-prefer ipv4 init-addr none
```

- `nameserver` 是 emfor 的 DNS server（`[dns] listen`，預設 5300 port），不是主機的 resolver。
- `accepted_payload_size 4096` 讓 backend 很多的服務的回答可以走 UDP 送到（HAProxy 預設只要 512 bytes）。
- `hold valid 1s` 讓 HAProxy 一秒後就採用新的回答；其他的 `hold` 維持預設，下面會說明為什麼重要。
- `1-4` 是**槽**的數量，也就是 HAProxy 最多能用的 backend 數。backend 比槽多的服務會被截斷（測試：三個 backend 放進四個槽，用到三個）。請設得比服務的 backend 數大。
- `_http._tcp.api.svc.emfor` 是 SRV 名稱；emfor 會回答 `_<任意>._tcp.<服務名稱>`（[SRV 記錄](#srv-記錄)），port 來自每一筆記錄。數個 backend 用同一個位址、不同 port 時要有 `allow-dup-ip`，`init-addr none` 讓 HAProxy 在第一次回答之前就能啟動。
- 不需要 `check`：沒通過 emfor 檢查的 backend 不在回答裡。你可以再加 HAProxy 自己的 `check` 當第二意見。

HAProxy 怎麼處理這個回答，用 HAProxy 3.4.7 與 2.8.31 測過：

- **健康狀態與 `drain = true`**（[第 5 章](05-backend-selection.md#drain-一個-backend)）會在幾秒內傳到 HAProxy，和 nginx 一樣：那台 backend 收不到請求，恢復或取消 drain 後又會收到。HAProxy 不用 reload。
- **不使用優先序。** `balance = "failover"` 時，HAProxy 把請求送給兩台 backend（40 個中 20 對 20），不是只送第一台。`locality` 與 `fastest` 用 priority 0 和 1 排序，也是一樣。不要指望 SRV 回答裡的備援；備援請用 HAProxy 自己的 `backup`，或用上面會遵守優先序的 nginx 做法。
- **不使用權重。** `balance = "weighted"`、權重 3 與 1 時，80 個請求是 40 對 40；在 template 上加 `weight 1` 或 `ignore-weight` 也一樣。
- **emfor 沒有回答時**（停掉或重新啟動）：HAProxy 依 `hold` 的時間保留它的 server。用上面的設定，emfor 停掉 4 秒後，所有請求仍然成功。
- **emfor 回 `SERVFAIL` 時**（沒有 backend 可給，或每個 backend 都 drain）：HAProxy 先保留 server 一陣子，然後丟掉，之後每個請求都得到 `503`；測試中在 10 到 30 秒之間就沒了，這是預設的 `hold other`（30 秒）。同樣的情況 nginx 會一直沿用最後的清單。

哪一種比較對，取決於 emfor 說沒有 backend 時你希望發生什麼。`hold` 的值是 HAProxy 的，這裡沒有調校過。

[tests/haproxy/](../../tests/haproxy/run.sh) 在指定的 HAProxy 映像上執行這些檢查（`tests/haproxy/run.sh haproxy:2.8`），包含 HAProxy 不做的那兩件事，所以日後 HAProxy 開始遵守優先序或權重時，測試會失敗，提醒這一節要改。Integrations workflow 每週執行一次，emfor 的 DNS 或候選的程式有改動時也會執行。沒試過：HAProxy 的 `mode tcp`、到 backend 的 TLS、`resolvers` 有多個 name server、上述兩個以外的版本。


在 FreeBSD 15.1（arm64，套件裡的 HAProxy 3.4.4）上，同樣的檢查全部通過，包含不遵守優先序與權重那兩項，所以這一節在那裡照樣成立。和 nginx 一樣，`tests/haproxy/inside.sh` 是手動跑的，除了 HAProxy 套件之外不需要別的東西。FreeBSD 上沒試過：其他版本的 HAProxy。

## 快取

emfor 的短 TTL 讓故障的 backend 大約一秒內從回答中消失。快取會拉長這段時間：

- resolver 的最小快取 TTL 保持為 0（dnsmasq `min-cache-ttl`、unbound `cache-min-ttl`）。
- resolver 在 TTL 期間從快取回答重複的查詢，所以主機上所有程式在這段時間內拿到同一個 round-robin 答案。降低 `dns_ttl`（最低 `0s`）可以更頻繁地輪替。
- 負向回答（尚未設定的名稱）會被快取 `[dns] ttl` 的時間。
- 應用程式的 runtime 可能自己快取（例如 JVM 的 `networkaddress.cache.ttl`）；emfor 無法縮短這些快取。
