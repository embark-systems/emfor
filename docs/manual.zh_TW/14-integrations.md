# 14. 整合

[手冊](README.md) · 上一章：[13. 安全模型](13-security.md) · 下一章：[15. 升級](15-upgrading.md)

這一章說明怎麼讓 proxy、監控與防火牆跟著 emfor 走。每個做法都會列出 repository 裡測它的測試，以及那個測試寫明的版本。**寫這份手冊時，這些測試一個都沒有重跑**；下面的結果是 repository 的測試和前一版手冊的紀錄。

所有 proxy 做法都要用 emfor 的 DNS server 和 SRV 記錄（[第 10 章](10-dns.md#srv-記錄)），所以 emfor 的設定裡要有 `[dns]`。它們的限制都一樣：

- emfor 的檢查先發現變化，proxy 再問一次，proxy 才會看到（用下面的設定大約一秒）；
- emfor 停掉時 DNS 不會回答，各個 proxy 會先沿用最後一份清單一段時間；
- 服務沒有 backend 可給時（每個 backend 都被 drain，或在 `on_all_down = "closed"` 下全部 down），emfor 會回 SERVFAIL。

## nginx

開源版 nginx 1.27.3 以上可以從 SRV 回答取得 upstream：

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;    # emfor 的 DNS server，每秒問一次
resolver_timeout 1s;                          # 查詢失敗後很快再問

upstream api {
    zone api 64k;                              # 用 `resolve` 一定要有
    server api.svc.emfor service=http resolve; # 查 _http._tcp.api.svc.emfor
}

server {
    listen 80;
    location / { proxy_pass http://api; }
}
```

| | nginx |
| --- | --- |
| 健康狀態與 drain | 幾秒內跟上，不用 reload。已經開著的連線會繼續開著。 |
| 優先序（`failover`） | 會照做：優先序 0 的當 server，其他的當 `backup`。 |
| 權重（`weighted`） | 會照做（權重 3:1 時，80 個請求中有 60 個）。 |
| SERVFAIL | 沿用最後一份清單；emfor 恢復回答後，要等 `resolver_timeout` 才會跟上（預設 30 s，所以上面設成 `1s`）。 |
| `stream` 模組 | 同樣的 `upstream` 也能用在 TCP。 |
| 測試 | `tests/nginx/run.sh`、`tests/nginx/stream.sh`；nginx 1.30.6 與 1.31.8。 |

`resolver` 要直接指向 emfor，不要指向主機的 resolver。

## HAProxy

```text
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

| | HAProxy |
| --- | --- |
| 健康狀態與 drain | 幾秒內跟上，不用 reload。 |
| 優先序、權重 | **不會用**：`failover` 會兩台 backend 都送，權重 3:1 結果是 40:40。請改用 HAProxy 自己的 `backup` 和 `weight`。 |
| 槽 | `1-4` 是最多能用的 backend 數；請設得比服務的 backend 數大。 |
| emfor 停掉時 | 照 `hold` 的時間保留原本的 server。 |
| SERVFAIL | 過了 `hold other`（30 s）就丟掉 server：之後每個請求都會拿到 503。 |
| 到 backend 的 TLS | 加上 `ssl ... sni str(<name>)`；一定要設 `sni`，因為 SRV 目標的名稱不在憑證裡。 |
| name server | 只設一個：同一台主機上的 emfor。設了兩個而它們意見不同時，server 清單會每次查詢都在變；如果好幾個 backend 共用同一個 IP（同一台主機的不同埠），HAProxy 3.4 還會對少數請求回 503（槽位的埠變成 0；400 個請求一輪，6 輪都出現，每個 backend 各有 IP 的 6 輪沒有）。只設一個 name server 在這種情況下也不夠：讓一個 backend 每 1.5 秒停掉再啟動，共用 IP 時 7 輪各 100 個請求，每輪有 0 到 12 個請求在約 3 秒後回 503（6 輪有出現）；每個 backend 各有 IP 時，3 輪各只有 1 個請求失敗，是 backend 停掉的那一刻。FreeBSD 15.1（HAProxy 3.4.4）上同樣的測試，共用 IP 時 4 輪各 100 個請求，每輪有 14 到 38 個回 503；每個 backend 各有 IP 時，4 輪各只有 1 到 2 個失敗。跑的時候取樣 `show servers state`，看到 503 落在沒有位址、port 為 0 的 slot（`s4` 一直是，`s2` 在它的 backend 停掉時是），HAProxy 仍把它們當 running，log 對應 `SC--`。 |
| 測試 | `tests/haproxy/run.sh`、`tests/haproxy/more.sh`；HAProxy 3.4.7 與 2.8.31，以及 FreeBSD 15.1 上的 3.4.4。 |

## Caddy

```text
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

第二個 `resolvers`（在 `transport http` 底下）是給 `10-0-0-11.api.svc.emfor` 這類 SRV 目標名稱用的，Caddy 要連到這些名稱。少了它，Caddy 會去問主機的 resolver，那主機的 resolver 就得轉送這個 zone。`grace_period` 讓查詢失敗時保留上一份清單。測試：`tests/caddy/run.sh`，Caddy 2.11.9。

## Prometheus

### 抓取 emfor 找到的 backend

```yaml
scrape_configs:
  - job_name: api
    dns_sd_configs:
      - names: ["_http._tcp.api.svc.emfor"]
        type: SRV
        refresh_interval: 5s
```

`dns_sd_configs` 用的是主機的 resolver，所以主機的 resolver 必須轉送整個 zone（[第 10 章](10-dns.md#從主機的-resolver-轉送)）。檢查失敗或被 drain 的 backend 會**直接從目標裡消失**：它的 `up` 序列就此中斷，而不是變成 0。這種做法適合看「現在正在服務的 backend」；想看到 `up == 0`，請另外留一份靜態清單。測試：`tests/prometheus/run.sh`，Prometheus 3.16.0。

### 抓取 emfor 本身

```yaml
scrape_configs:
  - job_name: emfor
    static_configs:
      - targets: ["127.0.0.1:5301"]
```

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml) 有一組根據 emfor metrics 寫的告警規則：`EmforMetricsDown`、`EmforServiceAllDown`、`EmforBackendUnhealthy`、`EmforBackendFlapping`、`EmforBackendCheckErrors`、`EmforBackendNotChecked` 與 `EmforStoreCommitFailing`。store 快滿、plugin 和 peer 都沒有規則；需要的話，請用 `emfor_store_used_bytes`、`emfor_plugin_up` 和 `emfor_peer_up` 自己加。這些規則附有 `promtool` 單元測試，放在 `emfor-rules.test.yml`：

```sh
$ promtool check rules examples/prometheus/emfor-rules.yml
$ promtool test rules examples/prometheus/emfor-rules.test.yml
```

## pf（FreeBSD）

pf 可以用 `rdr` 規則把連線分散到一個表裡的位址。[examples/pf/emfor-pf-table.sh](../../examples/pf/emfor-pf-table.sh) 每秒更新一次這個表，讓它和 emfor 的 SRV 回答中優先序最低的那些一致：

```text
table <api> persist
rdr pass on $ext_if inet proto tcp from any to $vip port 80 -> <api> port 8000 round-robin
```

```sh
# emfor-pf-table.sh loop api.svc.emfor api
```

- 每個 backend 都必須用 `rdr` 規則裡的 port。
- backend 從表裡移除時，已經開著的連線不會被切斷。
- 不會用到權重；`failover` 只會把第一台 backend 放進表。
- 收到 SERVFAIL 時會清空這個表，pf 則會丟掉沒辦法導向的連線；把 `EMPTY` 設成一個沒有任何程式在聽的本機位址，這些連線就會很快失敗。emfor 沒回答時，表維持原樣。
- 測試：`tests/pf/run.sh`，FreeBSD 15.1（手動跑，不在 CI 裡）。IPv6 沒有測過。

## Nagios、Icinga 與其他監控

### 監看 emfor：check_emfor.sh

[examples/nagios/check_emfor.sh](../../examples/nagios/check_emfor.sh) 會讀 `emfor status --json`（需要 `jq`），再照 Nagios 外掛的慣例回報：

```console
$ check_emfor.sh -s web.svc.emfor
EMFOR WARNING - web.svc.emfor: 2 of 3 healthy (127.0.0.1:7002 WARNING - slow on 7002) | 'web.svc.emfor_healthy'=2;3;1;0;3
```

| 狀態 | 何時 |
| --- | --- |
| CRITICAL (2) | daemon 是 stale 或 stopped，或健康的 backend 少於 `-c` 個（預設 1）。 |
| WARNING (1) | 健康的 backend 少於 `-w` 個（預設：所有沒被 drain 的 backend）。 |
| UNKNOWN (3) | `emfor` 或 `jq` 跑不起來、讀不到 store，或沒有這個服務。 |

被 drain 的 backend 不列入計算。跑監控的使用者需要和應用程式一樣的 store 存取權限（[第 12 章](12-operating.md#誰能讀取-store)）。

### 把 Nagios 外掛當成檢查

`exec` 檢查是用 `EMFOR_IP` 和 `EMFOR_PORT` 把 backend 傳進去，不是用參數。[examples/nagios/emfor-exec.sh](../../examples/nagios/emfor-exec.sh) 會把它們轉成 `-H` 和 `-p`，再去執行外掛：

```toml
[services."web.svc.emfor"]
check = "exec"
backends = ["10.0.0.11:80", "10.0.0.12:80"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/emfor-exec.sh", "/usr/local/libexec/nagios/check_http", "-u", "/healthz"] }
```

外掛的結束狀態就是 emfor 的結果：0 是 up，1（WARNING）和 2（CRITICAL）是 down，3（UNKNOWN）和其他值是 error；輸出的第一行就是 reason。加 `--warning up` 會把 WARNING 算成 up；`--host-flag` 和 `--port-flag` 可以換掉旗標。兩支腳本都由 `tests/nagios/run.sh` 搭配替身外掛測過；真正的 Monitoring Plugins、Nagios、Icinga 和 Zabbix 都沒有試過。

## 舊式網管：SNMP 與 hook

emfor 沒有 SNMP agent 或 trap 發送功能，狀態變化時也沒有可以跑的指令。enterprise OID 要向 IANA 申請，MIB 要有人維護，SNMPv3 又要用 MD5、SHA 和 AES，這和[第 13 章](13-security.md#fips)對 emfor 所用演算法的說法對不起來。hook 則要先決定：指令卡住、失敗，或同一個一直震盪的 backend 讓它跑上千次，該怎麼辦。emfor 給你的是每一次狀態變化的穩定紀錄，小程式可以把它轉成對方系統要的東西。

**輪詢 `emfor status --json`。** 每個 backend 最近 20 次狀態變化都在 `history`（[第 12 章](12-operating.md#emfor-status)）。[examples/snmp/emfor-snmp-trap.sh](../../examples/snmp/emfor-snmp-trap.sh) 會記住它送過的最新一筆變化，對每一筆更新的變化用 net-snmp 的 `snmptrap` 送一個 SNMPv2c trap。用 cron 或 systemd timer 跑，間隔要短到 20 次變化發生不完：

```console
$ emfor-snmp-trap.sh -H nms.example.net -o 1.3.6.1.4.1.99999.1 -c secret
```

`1.3.6.1.4.1.99999` 代表**你自己的** enterprise number；trap 是 `OID.1`，帶服務、backend、舊狀態、新狀態和原因這幾個字串（`OID.2` 到 `OID.6`）。MIB 請自己照這些寫。第一次執行什麼都不送，只記下目前的位置。手冊在 Debian 12 container 裡用 net-snmp 5.9.3 的 `snmptrapd` 實際跑過：停掉再啟動一台 backend，每次變化各收到一個 trap，下一次執行不再重送。它沒有對別家的管理站送過 trap。daemon 重啟之後，每台 backend 都會從 `unknown` 變成 `healthy`，腳本會為這些變化各送一個 trap，接收端要預期這一波。community 字串是明文送出的；要用 SNMPv3 的話，請另外給 `snmptrap` 它自己的選項，例如放在 `snmp.conf`，並把腳本的參數改成對應的樣子。

**讀 log。** 設了 `[log] format = "json"`（[第 12 章](12-operating.md#json-行)），狀態變化就是一行 `event` 為 `backend_state_changed` 的 JSON。rsyslog 的 `omprog`，或讀 journal 的程式（`journalctl -u emfor -o cat -f`）可以每來一行就處理一行，沒有輪詢的延遲。讀設定檔之前的行是文字，程式要略過不是 JSON 的行。

**讓管理站來問。** net-snmp 的 `snmpd` 可以用 `extend` 把一個指令的輸出開放出去，管理站再用 SNMP GET 讀：

```text
extend emfor-web /usr/local/bin/check_emfor.sh -s web.svc.emfor
```

結果和輸出在 `NET-SNMP-EXTEND-MIB`（`nsExtendResult` 和 `nsExtendOutput1Line`）。`snmpd` 用它自己的使用者執行指令，這個使用者需要和應用程式一樣的 store 存取權限（[第 12 章](12-operating.md#誰能讀取-store)）。這一行在 Debian 12 container 裡對 `snmpd` 5.9.3 實際跑過（以 root 執行）：每台 backend 都健康時 `nsExtendResult` 是 0，唯一的 backend 停掉後是 2（CRITICAL）。`snmpd` 會把輸出保留 5 秒（`nsExtendCacheTime`），所以變化要過那段時間才看得到。一般的 Debian 安裝沒有這類名稱用的 `NET-SNMP-EXTEND-MIB` 檔案（它在 non-free 的 `snmp-mibs-downloader`），請直接讀數字 OID：`.1.3.6.1.4.1.8072.1.3.2.3.1.4`（結果）和 `.3.1.1`（輸出第一行），後面接名稱的長度與每個字元。

## 會查 SRV 記錄的程式

Go 的 `net.LookupSRV("http", "tcp", "api.svc.emfor")` 查的是 `_http._tcp.api.svc.emfor`，emfor 會回答。這種 client 會拿到每個候選和各自的 port，應該照優先序一個一個試。只拿第一筆記錄的 client 一樣會分散開來，因為記錄的順序會輪替。
