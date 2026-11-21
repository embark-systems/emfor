# 10. DNS

[手冊](README.md) · 上一章：[9. Client 函式庫](09-clients.md) · 下一章：[11. 聯防群組](11-peer-groups.md)

daemon 可以為它的服務回答 DNS 查詢，讓沒有 client 函式庫的程式（proxy、shell script、你無法修改的程式）也能拿到健康的 backend。它只回答自己的 zone，從不轉送其他名稱：它位在主機的 resolver 後面，而不是取代它。

```text
program --> host resolver (dnsmasq, unbound, systemd-resolved) --> other names: upstream DNS
                     |
                     +-- *.svc.emfor --> emfor on 127.0.0.1:5300
```

daemon 停止時，只有它 zone 裡的名稱無法解析。

## 啟用

```toml
[dns]
listen = ["127.0.0.1:5300"]
zone = "svc.emfor"
ttl = "1s"
rate_limit = 100

[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
```

沒有 `[dns]` table 時不會開啟任何 DNS socket。空的 `[dns]` table 時，每個鍵都是表中的預設值。

| 鍵 | 預設值 | 規則 |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5300"]` | 每個位址都聽 UDP 與 TCP。改變它需要重新啟動。 |
| `zone` | `svc.emfor` | 最多 242 個字元。不在 zone 之內的服務不會被回答；daemon 在啟動與 reload 時會為每一個這樣的服務發出警告。 |
| `ttl` | `1s` | 整數秒，0–3600。也是 SOA 的 minimum，所以也是負向回答被快取的時間。 |
| `rate_limit` | `100` | 每個 client 網段（IPv4 /24 或 IPv6 /56）每秒的 UDP 回答數，0–1,000,000；`0` 表示不限制。loopback 上的 client 一律不受限。 |

服務可以用 `dns_ttl = "0s"` 設定自己的 TTL（整數秒，0–3600；不能寫在 `[defaults]` 中）。

啟動時，DNS 聽在不是 loopback 的位址上時，daemon 會發出警告（`DNS listens on X, which is not a loopback address: ...`）；backend 使用超過一個 port 的每個服務也會有一則警告：
`service S has backends on different ports (8081, 8082); DNS A/AAAA answers carry no port, so use SRV records or an emfor client to get the port`。

要在 systemd 下聽 port 53，請給 unit `CAP_NET_BIND_SERVICE`（[第 4 章](04-installing.md#systemd-unit)）。

## 回答

回答來自 daemon 的記憶體，帶有 authoritative-answer（`aa`）旗標。

| 查詢 | 回答 |
| --- | --- |
| 服務的 A | 一個 IPv4 backend，依 `balance` 選出（[第 8 章](08-choosing-a-backend.md#dns-與-client-函式庫的比較)）。 |
| 服務的 AAAA | IPv6 backend 的同樣做法。 |
| A 或 AAAA，沒有該位址族的 backend | NOERROR，沒有記錄，附 SOA。 |
| 服務或 `_<anything>._tcp.<service>` 的 SRV | 每個候選一筆 SRV 記錄，兩種位址族都有，帶 port；位址放在 additional section。 |
| 服務的其他類型 | NOERROR，沒有記錄，附 SOA。 |
| 全部 down 的服務，`on_all_down = "closed"` | SERVFAIL。 |
| 對 backend 全部 drain 的服務（A 與 AAAA 看該位址族的 backend）查詢 A、AAAA 或 SRV，`open` 也一樣 | SERVFAIL（[第 8 章](08-choosing-a-backend.md#drain)）。 |
| `<ip-label>.<service>`（SRV 的 target） | backend 的位址，不論它是否健康。 |
| zone 的 SOA | `svc.emfor. 1 IN SOA ns.svc.emfor. hostmaster.svc.emfor. <serial> 3600 600 86400 1`（serial 是 daemon 的啟動時間）。 |
| 服務名稱的上層名稱，或 `_tcp.<service>` | NOERROR，沒有記錄，附 SOA。 |
| zone 中的其他名稱 | NXDOMAIN，附 SOA。 |
| zone 之外的名稱，或 class 不是 IN | REFUSED。 |
| daemon 第一批結果出來之前 | SERVFAIL（不會像 NXDOMAIN 那樣被快取）。 |
| 超過一個 question | FORMERR。opcode 不是 QUERY 時回 NOTIMP；EDNS 版本大於 0 時回 BADVERS。 |

名稱比對不分大小寫，並忽略結尾的點。

### SRV 記錄

| `balance` | Priority | Weight | 順序 |
| --- | --- | --- | --- |
| `failover` | 依設定檔順序 0、1、2… | 1 | 設定檔順序 |
| `weighted` | 0 | backend 的 `weight` | 設定檔順序 |
| `locality` | 在主機的 `site` 為 0，其他為 1（沒有任何一個符合時全為 0） | 1 | 輪替 |
| `fastest` | 在快的 pool 中為 0，不在為 1（pool 為空時全為 0） | 1 | 輪替 |
| `round-robin`、`sticky` | 0 | 1 | 輪替 |

有候選正在[慢啟動](08-choosing-a-backend.md#慢啟動)時，除了 `failover` 以外的每種方式都改給每筆記錄它的有效權重。

target 名稱是放在服務名稱之下的 backend IP：

- `api.svc.emfor` 之下的 `10.0.0.11` 是 `10-0-0-11.api.svc.emfor`；
- `2001:db8::11` 是 `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor`（八組全部寫出，每組四位數）。

走 UDP 時，回答不超過 client 以 EDNS 提出的大小（最多 1232 位元組），沒有 EDNS 時不超過 512 位元組。位址放不下時，就從 additional section 中省略；SRV 記錄放不下時，TC 旗標會讓 resolver 改用 TCP 再問一次。TCP 同時最多接受 128 條連線，5 秒內沒有送出查詢的連線會被關閉。

### 檢查

用 `dig` 直接查詢 daemon：

```sh
$ dig @127.0.0.1 -p 5300 +short api.svc.emfor A
$ dig @127.0.0.1 -p 5300 api.svc.emfor SRV
$ dig @127.0.0.1 -p 5300 10-0-0-11.api.svc.emfor A
```

在[第 2 章](02-quick-start.md)的快速入門中，一個 backend up、一個 down 時，daemon 的回答如下（用一個小型的 Python DNS client 檢查，因為測試容器裡沒有安裝 `dig`）：

| 查詢 | 回答 |
| --- | --- |
| `web.svc.emfor A` | `127.0.0.1`，TTL 1，設了 `aa` |
| `web.svc.emfor SRV` | `0 1 8080 127-0-0-1.web.svc.emfor.`，位址在 additional section |
| `_http._tcp.web.svc.emfor SRV` | 同一筆記錄 |
| `127-0-0-2.web.svc.emfor A`（down 的那個 backend） | `127.0.0.2` |
| `svc.emfor SOA` | SOA，TTL 1 |
| `nope.svc.emfor A` | NXDOMAIN，附 SOA |
| `example.com A` | REFUSED，沒有 `aa` |

### 速率限制

走 UDP 時，某個 client 網段在一秒內送出超過 `rate_limit` 個查詢，超過限制的回答會輪流是設了 TC 旗標的空回答（真正的 resolver 會因此改用不受限制的 TCP 再問）以及完全不回答。`emfor_dns_rate_limited_total{action="truncated"}` 與 `{action="dropped"}` 計算它們的數量。DNS 聽在不是 loopback 的位址上時，這個限制用來防範位址偽造；在 `127.0.0.1` 上它從不生效。

## 從主機的 resolver 轉送

從你的程式已經在用的 resolver 轉送這個 zone，而且只轉送這個 zone。以下片段就是 [docs/deploy/resolver/](../deploy/resolver/) 裡的那些。

**dnsmasq**，`/etc/dnsmasq.d/emfor.conf`，然後重新啟動 dnsmasq：

```text
server=/svc.emfor/127.0.0.1#5300
```

**unbound**（Linux），或 FreeBSD 的 `local_unbound`，寫在 `/var/unbound/conf.d/emfor.conf`：

```text
server:
    do-not-query-localhost: no
    domain-insecure: "svc.emfor"

forward-zone:
    name: "svc.emfor"
    forward-addr: 127.0.0.1@5300
```

兩行 `server:` 設定都是必要的：unbound 預設不查詢 localhost，而會做驗證的 unbound 會拒絕這個未簽章的 zone（SERVFAIL）。

**systemd-resolved**（systemd 246 或更新版本），`/etc/systemd/resolved.conf.d/emfor.conf`，然後
`systemctl restart systemd-resolved`：

```ini
[Resolve]
DNS=127.0.0.1:5300
Domains=~svc.emfor
```

**macOS**（僅供開發），檔案 `/etc/resolver/svc.emfor`：

```text
nameserver 127.0.0.1
port 5300
```

接著透過系統 resolver 測試：Linux 與 FreeBSD 用 `getent hosts api.svc.emfor`，macOS 用 `dscacheutil -q host -a name api.svc.emfor`。在 macOS 上，`dig` 會略過 `/etc/resolver`。

根據 [docs/deploy/dns-resolver.md](../deploy/dns-resolver.md)，dnsmasq 與 unbound 的片段在 Debian 容器中測試過（`tests/resolver/run.sh`），`local_unbound` 在 FreeBSD 15.1 上，systemd-resolved 在使用 systemd 259 的 Ubuntu VM 上，macOS 則是手動測試。這些都沒有為本手冊重新執行。

## 快取

emfor 的短 TTL 讓故障的 backend 大約一秒內從 DNS 回答中消失。快取會拉長這段時間：

- resolver 的最小快取 TTL 保持為 0（dnsmasq `min-cache-ttl`、unbound `cache-min-ttl`）。
- 會快取的 resolver 在 TTL 期間從快取回答重複的查詢，所以主機上每個程式在這段時間內都拿到同一個 backend。調低 `dns_ttl`（最低 `0s`）可以更頻繁地輪替。
- NXDOMAIN 與空回答也會被快取 `[dns] ttl` 的時間：剛加入的名稱在這段時間內可能仍是負向回答。
- SERVFAIL 只會被快取很短的時間或完全不快取，所以 `closed` 的服務在 backend 恢復後很快就能再次解析。
- 有些 runtime 會自己快取（JVM 的 `networkaddress.cache.ttl`）。emfor 無法縮短它。

## 用 DNS 還是 client 函式庫？

能用 client 函式庫時優先使用它：它不需要設定 resolver、會回傳 port、支援 `sticky`，而且在 daemon 重新啟動期間仍以 store 最後的狀態運作（由 `on_all_down` 決定），而 DNS 在 daemon 停止期間完全沒有回答。對於你無法修改的程式，以及會重新解析名稱的 proxy，DNS 是正確的選擇（[第 14 章](14-integrations.md)）。
