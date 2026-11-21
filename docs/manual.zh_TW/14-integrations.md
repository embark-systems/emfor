# 14. 整合

[手冊](README.md) · 上一章：[13. 安全模型](13-security.md) · 下一章：[15. 升級](15-upgrading.md)

怎麼讓 proxy、監控與防火牆跟著 emfor 走。每個做法都列出 repository 裡測試它的測試，以及那個測試寫明的版本。**這份手冊沒有重新執行其中任何一個測試**；下面的結果是 repository 的測試與先前的手冊所報告的。

所有 proxy 做法都使用 emfor 的 DNS server 與 SRV 記錄（[第 10 章](10-dns.md#srv-記錄)），所以 emfor 的設定裡需要 `[dns]`。它們有同樣的限制：

- emfor 的檢查發現變化、proxy 再問一次之後，proxy 才看到變化（用下面的設定大約一秒）；
- emfor 停止時，DNS 沒有回答，每個 proxy 會沿用最後的清單一段時間；
- 服務沒有東西可給時（每個 backend 都被 drain，或在 `on_all_down = "closed"` 下全部 down），emfor 回答 SERVFAIL。

## nginx

開源版的 nginx 1.27.3 或更新的版本可以從 SRV 回答取得 upstream：

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;    # emfor 的 DNS server，每秒問一次
resolver_timeout 1s;                          # 查詢失敗後不久就再問

upstream api {
    zone api 64k;                              # 用 `resolve` 就一定要有
    server api.svc.emfor service=http resolve; # 查 _http._tcp.api.svc.emfor
}

server {
    listen 80;
    location / { proxy_pass http://api; }
}
```

| | nginx |
| --- | --- |
| 健康狀態與 drain | 幾秒內跟上，不需要 reload。已開著的連線保持開著。 |
| 優先序（`failover`） | 會遵守：優先序 0 的是 server，其他是 `backup`。 |
| 權重（`weighted`） | 會遵守（權重 3:1 時 80 個請求中 60 個）。 |
| SERVFAIL | 沿用最後的清單；emfor 再度回答之後，經過 `resolver_timeout` 才跟上（預設 30 s，所以上面設成 `1s`）。 |
| `stream` 模組 | 同樣的 `upstream` 也能用於 TCP。 |
| 測試 | `tests/nginx/run.sh`、`tests/nginx/stream.sh`；nginx 1.30.6 與 1.31.8。 |

`resolver` 要直接指向 emfor，不是主機的 resolver。

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
| 健康狀態與 drain | 幾秒內跟上，不需要 reload。 |
| 優先序、權重 | **不使用**：`failover` 會送給兩台 backend，權重 3:1 得到 40:40。請用 HAProxy 自己的 `backup` 與 `weight`。 |
| 槽 | `1-4` 是它最多能用的 backend 數；請設得比服務的 backend 數大。 |
| emfor 停止時 | 依 `hold` 的時間保留它的 server。 |
| SERVFAIL | 經過 `hold other`（30 s）後丟掉 server：每個請求都得到 503。 |
| 到 backend 的 TLS | 加上 `ssl ... sni str(<name>)`；一定要設 `sni`，因為 SRV 目標的名稱不在憑證裡。 |
| name server | 只用一個：同一台主機上的 emfor。兩個意見不同時，server 清單會隨每次查詢而變。 |
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

第二個 `resolvers`（在 `transport http` 底下）是給 `10-0-0-11.api.svc.emfor` 這類 SRV 目標名稱用的，Caddy 要連線到它們；少了它，Caddy 會問主機的 resolver，那主機的 resolver 就必須轉送這個 zone。`grace_period` 在查詢失敗時保留上一份清單。測試：`tests/caddy/run.sh`，Caddy 2.11.9。

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

`dns_sd_configs` 使用主機的 resolver，它必須轉送整個 zone（[第 10 章](10-dns.md#從主機的-resolver-轉送)）。檢查失敗或被 drain 的 backend **不再是目標**：它的 `up` 序列結束，而不是變成 0。這適合用來表示「現在正在服務的 backend」；要看到 `up == 0`，請另外保留一份靜態清單。測試：`tests/prometheus/run.sh`，Prometheus 3.16.0。

### 抓取 emfor 本身

```yaml
scrape_configs:
  - job_name: emfor
    static_configs:
      - targets: ["127.0.0.1:5301"]
```

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml) 提供以 emfor 的 metrics 為依據的告警規則：`EmforMetricsDown`、`EmforServiceAllDown`、`EmforBackendUnhealthy`、`EmforBackendFlapping`、`EmforBackendCheckErrors`、`EmforBackendNotChecked` 與 `EmforStoreCommitFailing`。沒有 store 快滿、plugin 或 peer 的規則；需要的話，請自己以 `emfor_store_used_bytes`、`emfor_plugin_up` 與 `emfor_peer_up` 加上。這些規則附有 `emfor-rules.test.yml` 裡的 `promtool` 單元測試：

```sh
$ promtool check rules examples/prometheus/emfor-rules.yml
$ promtool test rules examples/prometheus/emfor-rules.test.yml
```

## pf（FreeBSD）

pf 可以用 `rdr` 規則把連線分給一個表；[examples/pf/emfor-pf-table.sh](../../examples/pf/emfor-pf-table.sh) 每秒一次，讓這個表等於 emfor 的 SRV 回答中優先序最低的那些：

```text
table <api> persist
rdr pass on $ext_if inet proto tcp from any to $vip port 80 -> <api> port 8000 round-robin
```

```sh
# emfor-pf-table.sh loop api.svc.emfor api
```

- 每個 backend 都必須使用 `rdr` 規則的 port。
- backend 離開表時，已開著的連線不會被切斷。
- 不使用權重；`failover` 只把第一台 backend 放進表。
- 收到 SERVFAIL 時表會被清空，而 pf 會丟掉它無法導向的連線；把 `EMPTY` 設成一個沒有東西在聽的本機位址，讓這些連線很快失敗。emfor 沒有回答時，表維持原樣。
- 測試：`tests/pf/run.sh`，FreeBSD 15.1（手動執行，不在 CI 裡）。IPv6 沒有跑過。

## Nagios、Icinga 與其他監控

### 監看 emfor：check_emfor.sh

[examples/nagios/check_emfor.sh](../../examples/nagios/check_emfor.sh) 讀取 `emfor status --json`（需要 `jq`），並依 Nagios 外掛的慣例回報：

```console
$ check_emfor.sh -s web.svc.emfor
EMFOR WARNING - web.svc.emfor: 2 of 3 healthy (127.0.0.1:7002 WARNING - slow on 7002) | 'web.svc.emfor_healthy'=2;3;1;0;3
```

| 狀態 | 何時 |
| --- | --- |
| CRITICAL (2) | daemon 是 stale 或 stopped，或健康的 backend 少於 `-c` 個（預設 1）。 |
| WARNING (1) | 健康的少於 `-w` 個（預設：每一個沒有被 drain 的 backend）。 |
| UNKNOWN (3) | `emfor` 或 `jq` 無法執行、讀不到 store，或沒有這個服務。 |

被 drain 的 backend 不計入。監控所用的使用者需要和應用程式一樣的 store 存取權限（[第 12 章](12-operating.md#誰能讀取-store)）。

### 把 Nagios 外掛當成檢查

`exec` 檢查把 backend 放在 `EMFOR_IP` 與 `EMFOR_PORT` 傳入，而不是當作參數。[examples/nagios/emfor-exec.sh](../../examples/nagios/emfor-exec.sh) 把它們轉成 `-H` 與 `-p`，再執行外掛：

```toml
[services."web.svc.emfor"]
check = "exec"
backends = ["10.0.0.11:80", "10.0.0.12:80"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/emfor-exec.sh", "/usr/local/libexec/nagios/check_http", "-u", "/healthz"] }
```

外掛的結束狀態就是 emfor 的結果：0 是 up，1（WARNING）與 2（CRITICAL）是 down，3（UNKNOWN）與其他值是 error；輸出的第一行是 reason。`--warning up` 讓 WARNING 算成 up；`--host-flag` 與 `--port-flag` 用來換掉旗標。兩支腳本都由 `tests/nagios/run.sh` 以替身外掛測試；真正的 Monitoring Plugins、Nagios、Icinga 與 Zabbix 都沒有試過。

## 會查 SRV 記錄的程式

Go 的 `net.LookupSRV("http", "tcp", "api.svc.emfor")` 查的是 `_http._tcp.api.svc.emfor`，emfor 會回答。這樣的 client 會拿到每個候選與它的 port，應該依優先序逐一嘗試。只取第一筆記錄的 client 一樣會分散，因為順序會輪替。
