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

SRV 回答可以比查詢大上許多倍，所以其他主機連得到的 DNS server 可能被拿來用偽造來源的 UDP 查詢灌爆第三方。因此 server 會依 client 網段（IPv4 /24 或 IPv6 /56）計算 UDP 回應，每秒最多送出 `[dns] rate_limit` 個（預設 100）。超過之後，每兩個回應中有一個改成空的、設了 TC 旗標的回應：它不比查詢大（查詢沒有 EDNS 時大小相同），正常的 client 會改用 TCP 重試；其餘不回應。loopback 位址上的 client 與 TCP 一律不受限，所以預設的設定（`listen` 在 127.0.0.1，由本機 resolver 查詢）不受影響。`emfor_dns_rate_limited_total` 計算被限制的回應；如果站內轉送到 emfor 的 resolver 出現在這裡，就調高 `rate_limit`，或設為 `0` 關閉限制。

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
| A / AAAA / SRV，沒有健康的 backend 且 [`on_all_down = "closed"`](05-backend-selection.md#沒有任何-backend-健康時) | `SERVFAIL`。A 與 AAAA 的「沒有健康的 backend」是依位址族分開算的。 |
| A / AAAA / SRV，服務的每個 backend 都被 drain | `SERVFAIL`，不論 `on_all_down` 是什麼（[第 5 章](05-backend-selection.md#drain-一個-backend)）。 |
| zone 之下的任何名稱，在 daemon 還沒有第一份狀態之前（剛啟動的那一小段時間） | `SERVFAIL`：還不知道有哪些名稱。 |
| SRV，某個 backend 的目標名稱會超過 255 位元組 | `SERVFAIL`。 |
| 服務或 `_<任意>._tcp.<服務>` 的 SRV | 每個候選一筆 SRV 記錄，位址放在 additional section。 |
| 服務的其他類型 | `NOERROR`，沒有記錄。 |
| zone 之下、但不是服務的名稱 | `NXDOMAIN`。 |
| zone 之外的名稱，或 class 不是 `IN` | `REFUSED`。 |
| EDNS 版本大於 0 | `BADVERS`。 |

TTL 是服務的 `dns_ttl`（[第 3 章](03-configuration.md#servicesname)），否則為 `[dns] ttl`（[第 3 章](03-configuration.md#dns)；預設 1 秒）。zone 有一筆合成的 SOA，其負向快取 TTL 也是 `[dns] ttl`。

## SRV 記錄

SRV 回答帶有每個 backend 的 port，並列出所有候選，選法與 A、AAAA 回答相同：最近一次檢查通過的主要 backend；沒有的話，最近一次檢查通過的[備援](05-backend-selection.md#備援-backend)；再沒有的話，全部健康的主要 backend 都在失敗時列出它們全部（見[檢查開始失敗的 backend](05-backend-selection.md#檢查開始失敗的-backend)），否則列出全部健康的備援。只要有健康的 backend，同一個回答就不會同時列出主要與備援；沒有任何健康且 `on_all_down` 為 `open` 時，列出 open 的候選（見[第 5 章](05-backend-selection.md#沒有任何-backend-健康時)）：先是 `unknown` 的 backend（最近一次檢查通過的優先），沒有才列出全部。IPv4 與 IPv6 backend 一起列出。Go 的 `net.LookupSRV("http", "tcp", "api.svc.emfor")` 會查詢 `_http._tcp.api.svc.emfor`，emfor 會回答（其他會組出這種名稱的 client 沒有試過）。

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

透過 UDP 時，回應大小不超過 client 以 EDNS 宣告的大小，但 emfor 把它限制在 1232 位元組以內（宣告 4096 的 client 只得到 1232），沒有 EDNS 時為 512 位元組。位址放不下時就省略位址；連 SRV 記錄都放不下時，回應會被截斷（TC），resolver 會改用 TCP 重試。測試中（IPv4 backend、短的服務名稱）在 1232 位元組內，帶位址可放 19 個 backend、不帶位址可放 27 個；512 位元組時分別是 8 個與 11 個。40 個 backend、client 宣告 4096 時，UDP 回應有 27 筆記錄並設了 TC，走 TCP 則 40 筆全到。個別 client 收到 TC 之後的行為，只試過 nginx 與 HAProxy，範圍以下面各節為準。名稱長度接近 255 位元組上限時，連單一 A 記錄或 SOA 都可能超過 512 位元組，這種回應同樣會被截斷。

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
resolver_timeout 1s;                          # 查詢失敗後多久再問一次

upstream api {
    zone api 64k;                              # 用 `resolve` 就一定要有
    server api.svc.emfor service=http resolve; # 查 _http._tcp.api.svc.emfor
}

server {
    listen 80;
    location / { proxy_pass http://api; }
}
```

`resolver` 直接指向 emfor（`[dns] listen`，預設 5300 port），不是主機的 resolver：nginx 有自己的 resolver 快取，`valid=1s` 讓它每秒重新問一次。`resolver_timeout 1s` 在查詢失敗之後才有作用，例如 emfor 回答 `SERVFAIL` 時：nginx 要等這麼久才會再問，預設是 30 秒（見下）。`service=http` 只是名稱的前綴（`_http._tcp`）；emfor 會回答 `_<任意>._tcp.<服務名稱>`（[SRV 記錄](#srv-記錄)），所以 port 來自每一筆記錄。

nginx 怎麼處理這個回答，以下用 nginx 1.30.6（stable）與 1.31.8（mainline）測過：

- **健康狀態**：檢查失敗的 backend 不在回答裡，nginx 就不再送請求給它；它恢復後又回來。測試對每個變化最多等 10 秒；檢查間隔 200 毫秒時大約花 3 秒。
- **`drain = true`**（[第 5 章](05-backend-selection.md#drain-一個-backend)）：改 emfor 的設定並 reload emfor；不用動 nginx，那台 backend 不再收到新的請求。正在 backend 上跑的請求會跑完，nginx 對它開著的連線也維持開著：見[用 drain 做滾動部署](08-operations.md#用-drain-做滾動部署)（`tests/deploy`）與下面的 stream 測試。
- **優先序**：nginx 把 priority 數字最小的記錄當 server，其他的當 `backup`。`balance = "failover"` 時，第一台健康的 backend 是唯一的 server，第二台是它的備援；`locality` 與 `fastest` 也給 priority 0 與 1（見上面的 SRV 表），但只有 `failover` 對 nginx 跑過。
- **`balance = "weighted"` 的權重**會被遵守：權重 3 與 1 時，80 個請求中有 60 個送到第一台。（nginx 的文件沒有說它會用 SRV 的權重。）
- **emfor 無法回答時**：服務沒有 backend 可以給（`on_all_down = "closed"` 且全部不健康，或每個 backend 都 drain），emfor 回 `SERVFAIL`；nginx 會記錄 `could not be resolved`，並**沿用它最後一次拿到的清單**，繼續送請求給那些 backend。停掉的 emfor 根本不會回任何答案，nginx 的查詢是逾時而不是收到 `SERVFAIL`；這個情況沒有試過（測試只 drain 過所有 backend），nginx 對逾時的處理可能和對 `SERVFAIL` 的不同。對 nginx 的用戶端來說，要靠 nginx 自己的 `proxy_next_upstream` 與 `max_fails` 才會失敗。**emfor 再度回答時，nginx 要等 `resolver_timeout` 之後才會跟上**，預設是 30 秒：測試中用預設值，emfor 再度回答 5 秒後 nginx 仍是舊清單，大約 26 秒後才跟上；設成 `resolver_timeout 1s` 則大約一秒就跟上（設 5 秒約 2 秒，設 10 秒約 7 秒）。在這個狀態下你做的改動，例如 drain，也同樣要等那麼久才會到 nginx。所以食譜設 `resolver_timeout 1s`。
- **HTTPS upstream**：記錄的目標是像 `10-0-0-11.api.svc.emfor` 的名稱。要把 `proxy_ssl_name`（並設 `proxy_ssl_server_name on`）設成 backend 憑證上的名稱。這一項沒有試過。

**TCP，用 `stream` 模組。** 同樣的 `upstream` 在 `stream` 區塊裡也能用，適合資料庫或其他任何 TCP 服務：

```nginx
stream {
    resolver 127.0.0.1:5300 valid=1s ipv6=off;
    resolver_timeout 1s;

    upstream db {
        zone db 64k;
        server db.svc.emfor service=pgsql resolve;
    }

    server {
        listen 5432;
        proxy_pass db;
    }
}
```

`tests/nginx/stream.sh` 用兩台回答 HTTP 的 backend（TCP proxy 會原樣轉送）檢查過（用的是 `api.svc.emfor` 與 `service=http`，不是上面的名稱），nginx 1.30.6 與 1.31.8。health、`drain`、`failover`（第二台是備援）與權重的行為和 `http` 相同，`SERVFAIL` 也相同，包含之後的 `resolver_timeout`。TCP 特有的是：

- **已經開著的連線不受影響。** backend 被 drain 時，一個 6 秒才傳完的回應，在連到它的連線上仍然完整收到（1200 個位元組收到 1200 個），新的連線則去了另一台。backend 如果是被停掉，同一條連線就被切斷（1200 個位元組只收到 160 個）。所以 drain 之後，連到那台 backend 的長連線要等用戶端或 backend 自己關閉，沒有東西會把它們移走。
- **連到 emfor 仍然列出、但已經停掉的 backend 的新連線，會改試下一台**：`proxy_next_upstream` 在 `stream` 裡預設是開的，backend 停掉期間建立的 25 條連線，失敗了 0 條。

沒試過：UDP（`listen ... udp`）、`ssl_preread`、`proxy_protocol`、`proxy_timeout`（閒置的連線在預設的十分鐘後被關閉，這是 nginx 文件說的）、nginx Plus。

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


在 FreeBSD 15.1（arm64，套件裡的 nginx 1.30.6）上，同樣的檢查全部通過，包含較舊的 nginx 的情況，食譜不需要改。nginx 套件讀的是 `/usr/local/etc/nginx/nginx.conf`，不是 `/etc/nginx`。Docker 版的測試本身不在那裡執行；腳本（`tests/nginx/inside.sh`）是手動跑的，需要 Linux 容器本來就有的三樣東西：`dig`（用 `drill` 包一層就行）、loopback 上的位址 `127.0.0.2`（`ifconfig lo0 alias 127.0.0.2/32`；FreeBSD 的 loopback 只有 `127.0.0.1`，較舊 nginx 那項檢查會把一個 backend 綁在 `127.0.0.2`），以及讓 `nginx` 讀 `/etc/nginx/nginx.conf`。`stream` 的檢查（`tests/nginx/stream.sh`）也在那裡手動跑過、通過，只多一行：FreeBSD 的套件把 `stream` 建成動態模組，所以 `nginx.conf` 最前面要有 `load_module /usr/local/libexec/nginx/ngx_stream_module.so;`（沒有它，`nginx -t` 會說 `unknown directive "stream"`）。`SERVFAIL` 之後的延遲在那裡也一樣：用預設 `resolver_timeout` 約 25 秒，設 `resolver_timeout 1s` 約一秒。

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
- `accepted_payload_size 4096` 告訴 emfor，HAProxy 接受比預設 512 bytes 更大的 UDP 回答；emfor 把它限制在 1232 bytes（[SRV 記錄](#srv-記錄)：帶位址 19 個 backend、不帶位址 27 個）。HAProxy 搭配超過 27 個 backend 沒有試過，HAProxy 之後是否改用 TCP 重試也沒有試過。
- `hold valid 1s` 讓 HAProxy 一秒後就採用新的回答；其他的 `hold` 維持預設，下面會說明為什麼重要。
- `1-4` 是**槽**的數量，也就是 HAProxy 最多能用的 backend 數。backend 比槽多的服務會被截斷（測試：三個 backend 放進四個槽，用到三個）。請設得比服務的 backend 數大。
- `_http._tcp.api.svc.emfor` 是 SRV 名稱；emfor 會回答 `_<任意>._tcp.<服務名稱>`（[SRV 記錄](#srv-記錄)），port 來自每一筆記錄。數個 backend 用同一個位址、不同 port 時要有 `allow-dup-ip`，`init-addr none` 讓 HAProxy 在第一次回答之前就能啟動。
- 不需要 `check`：沒通過 emfor 檢查的 backend 不在回答裡。你可以再加 HAProxy 自己的 `check` 當第二意見。

HAProxy 怎麼處理這個回答，用 HAProxy 3.4.7 與 2.8.31 測過：

- **健康狀態與 `drain = true`**（[第 5 章](05-backend-selection.md#drain-一個-backend)）會在幾秒內傳到 HAProxy，和 nginx 一樣：那台 backend 收不到請求，恢復或取消 drain 後又會收到。HAProxy 不用 reload。
- **不使用優先序。** `balance = "failover"` 時，HAProxy 把請求送給兩台 backend（40 個中 20 對 20），不是只送第一台。`locality` 與 `fastest` 也用 priority 0 和 1 排序，應該有同樣的表現；這沒有跑過。不要指望 SRV 回答裡的備援；備援請用 HAProxy 自己的 `backup`，或用上面會遵守優先序的 nginx 做法（以 `failover` 試過）。
- **不使用權重。** `balance = "weighted"`、權重 3 與 1 時，80 個請求是 40 對 40；在 template 上加 `weight 1` 或 `ignore-weight` 也一樣。
- **emfor 沒有回答時**（停掉或重新啟動）：HAProxy 依 `hold` 的時間保留它的 server。用上面的設定，emfor 停掉 4 秒後，所有請求仍然成功。
- **emfor 回 `SERVFAIL` 時**（沒有 backend 可給，或每個 backend 都 drain）：HAProxy 先保留 server 一陣子，然後丟掉，之後每個請求都得到 `503`；測試中在 10 到 30 秒之間就沒了，這是預設的 `hold other`（30 秒）。同樣的情況 nginx 在測試觀察的那幾秒內沿用最後的清單；它會沿用多久沒有試過。

哪一種比較對，取決於 emfor 說沒有 backend 時你希望發生什麼。`hold` 的值是 HAProxy 的，這裡沒有調校過。

以下三種變化，是 `tests/haproxy/more.sh` 用 HAProxy 3.4.7 與 2.8.31 看到的：

- **`mode tcp`**（寫在 `defaults`，或寫在 frontend 與 backend）沒有任何差別：server 來自同樣的回答，檢查失敗或被 drain 的 backend 收不到連線，恢復後又會收到。
- **到 backend 的 TLS**：在 template 加上 `ssl`。`ssl verify none` 可以用。要檢查憑證，用 `ssl verify required ca-file <ca> sni str(<名稱>)`，它會用 `<名稱>` 檢查憑證：名稱對的時候兩台 backend 都有服務，名稱不在憑證裡就什麼都不服務。**一定要設 `sni`**：SRV 目標的名稱（`10-0-0-11.api.svc.emfor`）不會出現在憑證裡，而沒有 `sni` 時，3.4.7 會拒絕憑證，2.8.31 卻會接受任何由那個 CA 簽發的憑證，不管名稱是什麼。**不要依賴 `verifyhost`**：錯的 `verifyhost` 搭配對的 `sni`，兩個版本都照樣服務；對的 `verifyhost` 但沒有 `sni`，2.8.31 服務，3.4.7 拒絕。
- **只設一個 `nameserver`**：只寫一行 `nameserver`，指向和 HAProxy 同一台主機上的 emfor。HAProxy 會把每個查詢送給每一個 nameserver，並採用最先到的那個有效回答，不會把它們合併（測試中，兩台 emfor 回答相同時，停掉其中任何一個 HAProxy 仍然繼續服務，故障的 backend 也照樣離開清單）。兩個回答不同時（只有一台 drain 某個 backend），清單會隨著每次查詢而變：HAProxy 把某個 backend 移除再加回來，常常放進另一個槽位，同樣的設定每次執行服務的結果都可能不同（Linux 上 3.4.7 與 2.8.31 跑了很多次，FreeBSD 上 3.4.4 跑了 6 次）。請求也可能失敗：這種變動之後，有個槽位是 ready 的，位址對、埠卻是 0，送到它的請求大約 3 秒後得到 `503`（每 1.5 秒停掉再啟動一台 backend：兩個 nameserver 時，Linux 3.4.7 上 5 次執行中 5 次出現，只用一個 nameserver 時每次最多一個；但在 FreeBSD 上同樣的測試，只用一個 nameserver 也出現：3.4.4（套件）5 次執行每次 80 個請求中有 29 到 45 個，3.4.7（從 port 編的）有 21 到 28 個，所以不是這兩個版本之間的改變，第二個 nameserver 也不是唯一的原因；為什麼 FreeBSD 會出現而 Linux 容器不會，沒有找到原因）。這些都是手動看到的，不在測試裡，HAProxy 內部確切的原因也沒有找到。要高可用，請在每個 HAProxy 旁邊各跑一個 emfor，而不是給一個 HAProxy 多個 nameserver。硬要設兩個的話，它們的回答必須一致，`drain` 也一樣；`option redispatch` 能掩蓋失敗的請求，但不會修好那個槽位。`hold obsolete 30s` 會保留兩邊回答的聯集來讓結果穩定，代價是 drain 或移除要等那麼久才生效：不建議。

[tests/haproxy/](../../tests/haproxy/run.sh) 在指定的 HAProxy 映像上執行這些檢查（`tests/haproxy/run.sh haproxy:2.8`），包含 HAProxy 不做的那兩件事，所以日後 HAProxy 開始遵守優先序或權重時，測試會失敗，提醒這一節要改。Integrations workflow 每週執行一次，emfor 的 DNS 或候選的程式有改動時也會執行。沒試過：emfor 自己的 TLS 檢查與 HAProxy 並用、到 backend 的 client 憑證、`resolvers` 有超過兩個 name server、上述兩個以外的版本。


在 FreeBSD 15.1（arm64，套件裡的 HAProxy 3.4.4）上，同樣的檢查全部通過，包含不遵守優先序與權重那兩項，所以這一節在那裡照樣成立。和 nginx 一樣，`tests/haproxy/inside.sh` 是手動跑的，除了 HAProxy 套件之外不需要別的東西。`tests/haproxy/more.sh`（`mode tcp`、到 backend 的 TLS、兩個 name server）也在那裡手動跑過，結果與 3.4.7 相同，包含 `sni` 的那幾項。FreeBSD 上沒試過：其他版本的 HAProxy。

## 搭配 Caddy 使用

Caddy 的 `reverse_proxy` 可以用 `dynamic srv` 從 SRV 回答取得上游，所以 health 與 drain 幾秒內就會傳到 Caddy，不需要 reload：

```
:80 {
    reverse_proxy {
        dynamic srv {
            name _http._tcp.api.svc.emfor
            refresh 1s
            resolvers 127.0.0.1:5300
            grace_period 30s
        }
        transport http {
            resolvers 127.0.0.1:5300
        }
        lb_try_duration 2s
        lb_try_interval 100ms
    }
}
```

- **兩個 `resolvers`**：`dynamic srv` 裡的用來查 SRV；回答裡的目標是 `10-0-0-11.api.svc.emfor` 這樣的名稱，Caddy 要連線到它們，`transport http` 裡的就是為了這個。少了它，Caddy 會問主機的 resolver，主機的 resolver 不認得這個 zone 時，每個請求都會失敗（測試中 10 個有 10 個失敗，log 是 `no such host`）；主機的 resolver 已經[轉送這個 zone](#從主機的-resolver-轉送)的話就不需要它。
- **`refresh`** 是 Caddy 多久再問一次；測試用 1 秒。預設值比較長（我讀 Caddy 文件的理解是一分鐘；沒有試過），`refresh` 越長，drain 或故障傳到 Caddy 就越慢。
- **`grace_period`** 在查詢失敗時保留上一份清單。service 的 backend 全部被 drain、或在 `on_all_down = "closed"` 下全部故障時 emfor 回答 `SERVFAIL`（預設的 `open` 仍會回答）：有 `grace_period`，Caddy 繼續回答（10 個有 10 個）；沒有的話 Caddy 沒有上游，每個請求都失敗（10 個有 10 個）。nginx 在它的測試中不需要這種設定，自己就保留了上一份清單。
- **`lb_try_duration` 與 `lb_try_interval`** 讓 Caddy 重試，所以剛停掉、但在 emfor 與 Caddy 察覺前（約一秒）還在清單裡的 backend 不會丟請求（測試中 25 個請求丟 0 個）。沒有它們，Caddy 不會重試，25 個丟了 3 個。

Caddy 怎麼處理這個回答，以下是 `tests/caddy/run.sh` 用 Caddy 2.11.9 看到的：

- 檢查失敗的 backend 離開清單，恢復後回來；`drain = true` 的 backend 同樣離開，不需要 reload Caddy，也沒有請求遺失（需要上面的重試）。
- **不遵守權重**：3 比 1 的 service，兩台各分到約一半（一次執行中 80 個請求分到 41 與 39）。
- **預設策略不遵守優先序**：`failover` 的 service 也把請求送給備援。`lb_policy first` 則依回答的順序取 backend，回答是依優先序排序的，所以 `balance = "failover"` 配 `lb_policy first`，backend 健康時全部送給第一台，不健康時送給第二台，恢復後再回到第一台。

[tests/caddy/](../../tests/caddy/run.sh) 在指定的 Caddy 映像上執行這些檢查（`tests/caddy/run.sh caddy:2`）；Integrations workflow 每週執行一次，emfor 的 DNS 或候選的程式有改動時也會執行。沒試過：HTTPS 上游、Caddy 自己的主動健康檢查與 emfor 並用、`first` 與預設以外的負載平衡策略、多個 Caddy、2.11.9 以外的版本、FreeBSD。

## 搭配 Prometheus 使用

Prometheus 可以用 `dns_sd_configs`（型別 `SRV`）從 SRV 回答找到要抓取的目標：目標就是 emfor 某個 service 中健康的 backend，清單跟著檢查結果變動，不需要 reload Prometheus。

```yaml
scrape_configs:
  - job_name: api
    dns_sd_configs:
      - names: ["_http._tcp.api.svc.emfor"]
        type: SRV
        refresh_interval: 5s
```

`dns_sd_configs` 問的是主機的 resolver，沒有設定 name server 或埠的欄位（`promtool check config` 會拒絕 `server` 欄位），所以主機的 resolver 要把這個 zone 轉送給 emfor，做法見[從主機的 resolver 轉送](#從主機的-resolver-轉送)。要轉送的是整個 zone，不只是 SRV 查詢：回答裡的目標是 `10-0-0-11.api.svc.emfor` 這樣的名稱，Prometheus 每次抓取都會再解析一次，而 emfor 會回答它們。`instance` 標籤是這個名稱加埠，不是位址；`__meta_dns_srv_record_target` 與 `__meta_dns_srv_record_port` 可以用在 `relabel_configs`。SRV 記錄裡的埠就是 emfor 裡 backend 的埠，所以應用程式的 metrics 要在這個埠上；要用別的埠，就另外做一個 backend 是那個埠的 service。

目標是什麼，以下是 `tests/prometheus/run.sh` 用 Prometheus 3.16.0 看到的（`refresh_interval` 1 秒，emfor 是容器唯一的 resolver）：

- `round-robin` service 的兩台 backend 都是目標，而且 `up`。
- 檢查失敗的 backend **不再是目標**：它不是顯示成 down，而是它的 `up` 序列結束。所以 `up == 0` 對它永遠不會觸發；請改用 emfor 自己的 metrics 發警報（`emfor_backend_state`，見 [examples/prometheus/](../../examples/prometheus/emfor-rules.yml)）。
- 被 drain 的 backend 也不是目標，雖然它還在跑、`/metrics` 也回得出來：在[用 drain 做滾動部署](08-operations.md#用-drain-做滾動部署)期間，它的 metrics 不會被收集。
- `failover` service 會列出它的備援（優先序 1），Prometheus 取每一筆記錄，所以兩台都抓。`backup = true` 的 backend 只有在沒有任何主要 backend 健康時才是目標。
- 目標出現與消失的時間，大約是 `refresh_interval` 加上 emfor 看出變化所需的時間（`interval`、`fall`、`rise`）。

所以 emfor 的 service 很適合當作「現在正在服務的 backend」的來源。想不論健康狀況都抓每一台、並看到 `up == 0`，抓取請維持靜態清單或檔案式探索，用 emfor 發警報。

[tests/prometheus/](../../tests/prometheus/run.sh) 執行這些檢查（Integrations workflow 每週執行一次，emfor 的 DNS 或候選的程式有改動時也會執行）。在 FreeBSD 15.1 上，同一支腳本用套件的 Prometheus 2.55.1 手動執行也通過（nginx 1.30.6 也是套件；停掉 `local_unbound`，emfor 在 53 埠）。沒試過：emfor 前面放 dnsmasq 或 unbound（測試把 emfor 當作唯一的 resolver）、其他版本的 Prometheus、HTTPS 目標、很多個目標、預設 30 秒的 `refresh_interval`。

## 搭配 pf 使用（FreeBSD）

在 FreeBSD 上，pf 可以把連線分給一個位址表的成員，資料路徑上不需要任何 proxy：`rdr` 規則把連到某個位址的連線依序導向表裡的成員。emfor 沒有「回答改變時執行指令」的 hook，所以用一個小迴圈，每秒向 emfor 的 DNS server 問這個 service 的 SRV 記錄，讓表和 emfor 提供的一致：

```
table <api> persist
rdr pass on $ext_if inet proto tcp from any to $vip port 80 -> <api> port 8000 round-robin
```

[examples/pf/emfor-pf-table.sh](../../examples/pf/emfor-pf-table.sh) 就是那個迴圈（`emfor-pf-table.sh loop api.svc.emfor api`，以 root 執行；例如放在 `daemon(8)` 底下，這沒有試過）。它取優先序最低的 SRV 記錄，從目標名稱讀出位址（`10-0-0-11.api.svc.emfor` 是 `10.0.0.11`），集合改變時執行 `pfctl -t api -T replace`。它需要 `drill`，base system 就有。service 的所有 backend 必須是同一個埠，也就是 `rdr` 規則裡的那個。

以下是 `tests/pf/run.sh` 在 FreeBSD 15.1 上看到的（backend 與被導向的位址都在 loopback 上）：

- **Health**：檢查失敗的 backend 離開表，恢復後回來；**`drain = true`** 同樣讓它離開（[第 5 章](05-backend-selection.md#drain-一個-backend)）。
- **已經開著的連線在 backend 離開表時不會被切斷**，因為 pf 保留了它的 state：連到被 drain 的 backend 的連線上，一個 6 秒才傳完的回應完整收到（1200 個位元組收到 1200 個），新連線則去了另一台。
- **優先序**：表裡只有優先序最低的，所以 `failover` 的 service 表裡是第一台，第一台故障時才是第二台。`backup = true` 的 backend 在主要 backend 健康時不在表裡。
- **不使用權重**：pf 的 `round-robin` 平均分配（權重 3 與 1，80 個連線分到 40 與 40）。
- **emfor 沒有東西可給時**（`SERVFAIL`：每個 backend 都被 drain，或在 `on_all_down = "closed"` 下全部故障）表會被清空，而 pf 會丟掉空的 `rdr` 表無法導向的連線：用戶端要等自己的逾時（loopback 與另一台主機上都看到）。把 `EMPTY` 設成主機上一個在 `rdr` 埠沒有東西在聽的位址，表就會放它而不是空的；另一台主機的用戶端，連到表裡一個沒有東西在聽的項目時被立刻拒絕（一秒內 `rc=7`）。腳本的 `EMPTY` 本身只檢查了它放進表裡的內容。
- **emfor 沒有回答時**（停掉、連不到）表維持原樣，emfor 回來後再跟上。
- 在 backend 故障到迴圈的下一輪之間（emfor 自己的檢查之後再加一秒），pf 仍然會把新連線送給它：從另一台主機看，連到沒有東西在聽的 backend 會被拒絕，所以用戶端應該重試。

沒試過：IPv6（腳本讀得懂 IPv6 目標，但沒有跑過）、`source-hash` 與 `sticky-address`、真實介面上很多用戶端的 `rdr`（另一台主機的檢查是從一台機器手動做的：被拒絕與被丟掉的連線）、迴圈用的 `daemon(8)` 或 rc script、其他防火牆。`tests/pf/run.sh` 需要 root 與有 pf 模組的 FreeBSD；它載入自己的規則並啟用 pf，所以 pf 已經開著時它會拒絕執行。它不在 CI 裡。

## 自己會查 SRV 記錄的 client

有些 client 自己會用 SRV 記錄找伺服器，所以不需要 proxy：它們查 `_<service>._tcp.<名稱>`，emfor 用 `_<任意>._tcp.<service>` 回答健康的 backend、它們的埠，以及 [SRV 記錄](#srv-記錄)那一節的優先序與權重。檢查失敗或被 drain 的伺服器不在回答裡。它們問的是主機的 resolver，所以主機的 resolver 要把這個 zone 轉送給 emfor（[從主機的 resolver 轉送](#從主機的-resolver-轉送)）；下面的測試是讓 emfor 當容器唯一的 resolver，聽 53 埠。只有 `_tcp` 的名稱會被回答：`_udp` 的名稱得到 `NXDOMAIN`。

- **Kerberos（MIT）**：`krb5.conf` 設 `dns_lookup_kdc = true`，realm 設成 service 名稱的大寫（service `kdc.svc.emfor` 對應 `KDC.SVC.EMFOR`），`kinit` 先問 `_kerberos._udp.KDC.SVC.EMFOR`（如上所說沒有答案），再問 `_kerberos._tcp.KDC.SVC.EMFOR`，然後用 TCP 連到兩台 KDC。檢查失敗的 KDC 沒有被提供，`kinit` 只去另一台。用 `balance = "failover"` 時，回答裡有優先序 0 和 1，`kinit` 先試主要那台。emfor 的 `tcp` 檢查只看 TCP 埠。
- **OpenLDAP**：`ldapsearch -H 'ldap:///dc=ldap%2Cdc=svc%2Cdc=emfor'`（沒有主機，只有 DN）會讓 libldap 去問 `_ldap._tcp.ldap.svc.emfor`，並使用回答裡的伺服器。**DN 裡的逗號要寫成 `%2C`**：用一般的逗號時 `ldapsearch` 會說 `Could not parse LDAP URI(s)`。檢查失敗的伺服器沒有被使用（3 次嘗試 3 次都去了另一台）。
- **MongoDB**：`mongodb+srv://db.svc.emfor/` 讓 PyMongo 拿到 `_mongodb._tcp.db.svc.emfor` 的兩台伺服器。emfor 沒有 TXT 記錄，對 `mongodb+srv` 也會發出的 TXT 查詢回答 `NOERROR` 而沒有任何記錄，PyMongo 接受了。`mongodb+srv` 會把 TLS 打開：伺服器不講 TLS 時，在 URI 加 `tls=false`。檢查失敗的伺服器與被 drain 的伺服器都不在清單裡。

[tests/srv/](../../tests/srv/run.sh) 用 Debian bookworm 的 `krb5-user`（MIT Kerberos）、`ldap-utils`（OpenLDAP 2.5.13）與 `python3-pymongo`（3.11.0）檢查這些；伺服器只是單純的 listener，所以檢查的是 client 被告知、並且去連的是哪些伺服器。沒試過：它們後面真正的 KDC、LDAP 或 MongoDB 伺服器（`kinit` 以 `Cannot contact any KDC` 結束）、LDAPS、sssd（`ldap_uri = _srv_`）、nslcd、Samba、Heimdal、Active Directory 的 `_msdcs` 之類名稱、`svc.emfor` 以外的 realm 或網域（那需要它自己的 `[dns] zone`）、PyMongo 以外的驅動程式、其他版本。Integrations workflow 每週執行一次，emfor 的 DNS 程式或這個測試有改動時也會執行。

## 快取

emfor 的短 TTL 讓故障的 backend 大約一秒內從回答中消失。快取會拉長這段時間：

- resolver 的最小快取 TTL 保持為 0（dnsmasq `min-cache-ttl`、unbound `cache-min-ttl`）。
- resolver 在 TTL 期間從快取回答重複的查詢，所以主機上所有程式在這段時間內拿到同一個 round-robin 答案。降低 `dns_ttl`（最低 `0s`）可以更頻繁地輪替。
- 負向回答（尚未設定的名稱）會被快取 `[dns] ttl` 的時間。
- 應用程式的 runtime 可能自己快取（例如 JVM 的 `networkaddress.cache.ttl`）；emfor 無法縮短這些快取。
