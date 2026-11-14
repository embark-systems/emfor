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

## 快取

emfor 的短 TTL 讓故障的 backend 大約一秒內從回答中消失。快取會拉長這段時間：

- resolver 的最小快取 TTL 保持為 0（dnsmasq `min-cache-ttl`、unbound `cache-min-ttl`）。
- resolver 在 TTL 期間從快取回答重複的查詢，所以主機上所有程式在這段時間內拿到同一個 round-robin 答案。降低 `dns_ttl`（最低 `0s`）可以更頻繁地輪替。
- 負向回答（尚未設定的名稱）會被快取 `[dns] ttl` 的時間。
- 應用程式的 runtime 可能自己快取（例如 JVM 的 `networkaddress.cache.ttl`）；emfor 無法縮短這些快取。
