# 10. DNS

[手冊](README.md) · 上一章：[9. Client 函式庫](09-clients.md) · 下一章：[11. 聯防群組](11-peer-groups.md)

daemon 可以替自己的服務回答 DNS 查詢，這樣沒有 client 函式庫的程式（proxy、shell script、你改不了的程式）也能拿到健康的 backend。daemon 只回答自己的 zone，其他名稱一律不轉送：它是放在主機的 resolver 後面，不是拿來取代 resolver。

```text
program --> host resolver (dnsmasq, unbound, systemd-resolved) --> other names: upstream DNS
                     |
                     +-- *.svc.emfor --> emfor on 127.0.0.1:5300
```

daemon 停掉時，只有它 zone 裡的名稱會解析不了。

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

沒有 `[dns]` table 就不會開任何 DNS socket。如果 `[dns]` table 是空的，每個鍵都用表中的預設值。

| 鍵 | 預設值 | 規則 |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5300"]` | 每個位址都同時聽 UDP 和 TCP。改這個值要重新啟動。 |
| `zone` | `svc.emfor` | 最多 242 個字元。不在 zone 裡的服務不會回答；daemon 啟動和 reload 時，會對每個這樣的服務發出警告。 |
| `ttl` | `1s` | 整數秒，0–3600。這也是 SOA 的 minimum，所以也決定負向回答會被快取多久。 |
| `rate_limit` | `100` | 每個 client 網段（IPv4 /24 或 IPv6 /56）每秒最多幾個 UDP 回答，0–1,000,000；`0` 表示不限制。loopback 上的 client 一律不受限制。 |

服務可以用 `dns_ttl = "0s"` 設定自己的 TTL（整數秒，0–3600；不能寫在 `[defaults]` 裡）。

啟動時，如果 DNS 聽在不是 loopback 的位址上，daemon 會發出警告（`DNS listens on X, which is not a loopback address: ...`）；backend 用到不只一個 port 的服務，每個也都會有一則警告：
`service S has backends on different ports (8081, 8082); DNS A/AAAA answers carry no port, so use SRV records or an emfor client to get the port`。

要在 systemd 底下聽 port 53，請給 unit 加上 `CAP_NET_BIND_SERVICE`（[第 4 章](04-installing.md#systemd-unit)）。

## 回答

回答來自 daemon 的記憶體，會設 authoritative-answer（`aa`）旗標。

| 查詢 | 回答 |
| --- | --- |
| 服務的 A | 一個 IPv4 backend，照 `balance` 選出（[第 8 章](08-choosing-a-backend.md#dns-與-client-函式庫的比較)）。 |
| 服務的 AAAA | 同上，但選的是 IPv6 backend。 |
| A 或 AAAA，但沒有該位址族的 backend | NOERROR，沒有記錄，附 SOA。 |
| 服務或 `_<anything>._tcp.<service>` 的 SRV | 每個候選一筆 SRV 記錄，兩種位址族都有，帶 port；位址放在 additional section。 |
| 服務的其他類型 | NOERROR，沒有記錄，附 SOA。 |
| 全部 down 的服務，且 `on_all_down = "closed"` | SERVFAIL。 |
| 服務的 backend 全部被 drain（A 和 AAAA 只看該位址族的 backend），查詢 A、AAAA 或 SRV，`open` 也一樣 | SERVFAIL（[第 8 章](08-choosing-a-backend.md#drain)）。 |
| `<ip-label>.<service>`（SRV 的 target） | backend 的位址，不管健不健康。 |
| zone 的 SOA | `svc.emfor. 1 IN SOA ns.svc.emfor. hostmaster.svc.emfor. <serial> 3600 600 86400 1`（serial 是 daemon 的啟動時間）。 |
| 服務名稱的上層名稱，或 `_tcp.<service>` | NOERROR，沒有記錄，附 SOA。 |
| zone 裡的其他名稱 | NXDOMAIN，附 SOA。 |
| zone 以外的名稱，或 class 不是 IN | REFUSED。 |
| daemon 第一批結果出來之前 | SERVFAIL（不會像 NXDOMAIN 那樣被快取）。 |
| 超過一個 question | FORMERR。opcode 不是 QUERY 時回 NOTIMP；EDNS 版本大於 0 時回 BADVERS。 |

名稱比對不分大小寫，結尾有沒有點都一樣。

### SRV 記錄

| `balance` | Priority | Weight | 順序 |
| --- | --- | --- | --- |
| `failover` | 依設定檔順序 0、1、2… | 1 | 設定檔順序 |
| `weighted` | 0 | backend 的 `weight` | 設定檔順序 |
| `locality` | 在主機的 `site` 是 0，其他是 1（沒有任何一個符合時全部是 0） | 1 | 輪替 |
| `fastest` | 在快的 pool 裡是 0，不在是 1（pool 是空的時全部是 0） | 1 | 輪替 |
| `round-robin`、`sticky` | 0 | 1 | 輪替 |

有候選正在[慢啟動](08-choosing-a-backend.md#慢啟動)時，除了 `failover`，其他方式都會改成給每筆記錄它的有效權重。

target 名稱是把 backend 的 IP 放在服務名稱底下：

- `api.svc.emfor` 底下的 `10.0.0.11` 是 `10-0-0-11.api.svc.emfor`；
- `2001:db8::11` 是 `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor`（八組全部寫出來，每組四位數）。

走 UDP 時，回答的大小不會超過 client 用 EDNS 提出的大小（最多 1232 位元組）；沒有 EDNS 時不超過 512 位元組。位址放不下，就不放進 additional section；SRV 記錄放不下，就設 TC 旗標，讓 resolver 改用 TCP 再問一次。TCP 同時最多接受 128 條連線，5 秒內沒送出查詢的連線會被關掉。

### 檢查

用 `dig` 直接查 daemon：

```sh
$ dig @127.0.0.1 -p 5300 +short api.svc.emfor A
$ dig @127.0.0.1 -p 5300 api.svc.emfor SRV
$ dig @127.0.0.1 -p 5300 10-0-0-11.api.svc.emfor A
```

拿[第 2 章](02-quick-start.md)的快速入門來試，一個 backend up、一個 down，daemon 的回答如下（測試容器裡沒裝 `dig`，所以是用一個小的 Python DNS client 查的）：

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

走 UDP 時，如果某個 client 網段一秒內送了超過 `rate_limit` 個查詢，超出限制的部分會輪流處理：一次回設了 TC 旗標的空回答（真正的 resolver 收到後會改用 TCP 再問，TCP 不受限制），一次完全不回答。這兩種分別算在 `emfor_dns_rate_limited_total{action="truncated"}` 和 `{action="dropped"}`。DNS 聽在不是 loopback 的位址上時，這個限制用來防範偽造來源位址；在 `127.0.0.1` 上永遠不會生效。

## 從主機的 resolver 轉送

在你的程式本來就在用的 resolver 上設定轉送這個 zone，而且只轉送這個 zone。下面的片段就是 [docs/deploy/resolver/](../deploy/resolver/) 裡那幾份。

**dnsmasq**：寫在 `/etc/dnsmasq.d/emfor.conf`，然後重新啟動 dnsmasq：

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

`server:` 底下這兩行都不能少：unbound 預設不會查 localhost，而有開驗證的 unbound 會拒絕這個沒簽章的 zone（SERVFAIL）。

**systemd-resolved**（systemd 246 以上）：寫在 `/etc/systemd/resolved.conf.d/emfor.conf`，然後執行
`systemctl restart systemd-resolved`：

```ini
[Resolve]
DNS=127.0.0.1:5300
Domains=~svc.emfor
```

**macOS**（只適合開發用）：檔案 `/etc/resolver/svc.emfor`：

```text
nameserver 127.0.0.1
port 5300
```

接著透過系統的 resolver 測試：Linux 和 FreeBSD 用 `getent hosts api.svc.emfor`，macOS 用 `dscacheutil -q host -a name api.svc.emfor`。注意在 macOS 上，`dig` 不會看 `/etc/resolver`。

根據 [docs/deploy/dns-resolver.md](../deploy/dns-resolver.md)，dnsmasq 和 unbound 的片段在 Debian 容器裡測過（`tests/resolver/run.sh`），`local_unbound` 是在 FreeBSD 15.1 上測，systemd-resolved 是在用 systemd 259 的 Ubuntu VM 上測，macOS 則是手動測。這些都沒有為了這份手冊重跑。

## 快取

emfor 的 TTL 很短，所以故障的 backend 大約一秒內就會從 DNS 回答裡消失。快取會把這段時間拉長：

- resolver 的最小快取 TTL 請維持 0（dnsmasq 的 `min-cache-ttl`、unbound 的 `cache-min-ttl`）。
- 會快取的 resolver，在 TTL 內會直接用快取回答重複的查詢，所以這段時間裡，主機上每個程式拿到的都是同一個 backend。想輪替得更頻繁，就把 `dns_ttl` 調低，最低可到 `0s`。
- NXDOMAIN 和空回答也會被快取 `[dns] ttl` 這麼久：剛加進去的名稱，在這段時間內可能還是拿到負向回答。
- SERVFAIL 只會快取很短的時間，或根本不快取，所以 backend 恢復後，`closed` 的服務很快就能再解析。
- 有些 runtime 會自己快取（例如 JVM 的 `networkaddress.cache.ttl`）。這部分 emfor 沒辦法縮短。

## 用 DNS 還是 client 函式庫？

能用 client 函式庫就用 client 函式庫：不用設定 resolver、會回傳 port、支援 `sticky`，而且 daemon 重新啟動的期間，還能照 store 最後的狀態繼續運作（由 `on_all_down` 決定）；DNS 則是 daemon 一停就完全不回答。如果是你改不了的程式，或是會重新解析名稱的 proxy，那就適合用 DNS（[第 14 章](14-integrations.md)）。
