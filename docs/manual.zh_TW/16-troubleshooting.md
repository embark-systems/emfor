# 16. 疑難排解

[手冊](README.md) · 上一章：[15. 升級](15-upgrading.md) · 下一章：[17. 從 HAProxy 或 Consul 搬過來](17-migrating.md)

先跑這三個指令，大部分的問題都會在其中一個看出來：

```sh
# emfor check-config                       # 設定有效嗎？
# emfor status                             # daemon 有在跑嗎？它看到了什麼？
# emfor lookup <service> --explain         # 查詢為什麼回這個答案？
```

再看 log：`journalctl -u emfor`（systemd），或 tag 為 `emfor` 的 syslog（FreeBSD）。下面的 emfor 訊息是 emfor 0.7.0 在 Debian 測試容器裡印出來的；例外是 plugin 的 reason 取自程式碼，systemd 和 DNS 的症狀則取自 repository 的文件。

## daemon 無法啟動

| 訊息 | 原因 | 處理 |
| --- | --- | --- |
| `cannot open state store: /var/lib/emfor/state.mdb.writer: No such file or directory (os error 2)` | store 目錄不存在；daemon 不會自己建。 | `install -d -o emfor -g emfor -m 0755 /var/lib/emfor`（unit 的 `StateDirectory=` 會幫你做這件事）。 |
| `cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held` | 已經有另一個 daemon 在用這個 store。 | 把那個停掉，或給這個 daemon 另一個 `[store] path`。 |
| `cannot start metrics server: cannot listen on 127.0.0.1:8080 (tcp): Address already in use (os error 98)` | port 已經被佔用。 | 把 port 空出來，或改 `listen`。 |
| `invalid config /etc/emfor/emfor.toml: 2 error(s)` 以及後面的錯誤行 | 設定無效（結束代碼 2）。 | 跑 `emfor check-config`，一行一行修（見下）。 |
| `/etc/emfor/conf.d: writable by group or others (mode 0775); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/conf.d` | 設定檔或目錄可以被其他人修改。 | 照訊息裡寫的跑 `chmod`（或 `chown`）。 |
| `include_dir: cannot read directory /etc/emfor/conf.d: No such file or directory (os error 2)` | `include_dir` 指到一個不存在的目錄。 | 把目錄建起來，或拿掉 `include_dir`。 |
| `--check-config was replaced by "emfor check-config"` | 用的是舊的命令列寫法。 | 改用 `emfor check-config`。 |
| systemd：重複啟動後 unit 變成 `failed` | 一分鐘內啟動失敗了五次（`StartLimitBurst=5`）。 | 先排除原因，再執行 `systemctl reset-failed emfor`。 |

## 設定錯誤

`emfor check-config` 每個錯誤印一行，開頭是 key 的路徑；如果錯誤在 `conf.d` 的檔案裡，後面還會接上檔名：

```text
services."web.svc.emfor".timeout: timeout must not exceed interval (in /etc/emfor/conf.d/50-x.toml)
```

| 訊息 | 處理 |
| --- | --- |
| `defaults.interval: invalid duration "2", use e.g. "500ms" or "2s"` | 時間長度要加單位。 |
| `defaults.jitter: jitter above 50% or below 0%` | `jitter` 要在 `"0%"`–`"50%"` 之間。 |
| `services.Bad_Name: invalid service name: labels of [a-z0-9-], 1-63 characters each, 253 characters total` | 名稱只能用小寫的 DNS 格式。 |
| `backend "web1:80" must be an IP literal with port: IPv4:port or [IPv6]:port, port 1-65535` | backend 要寫 IP 位址，不能寫主機名稱。 |
| `duplicate backend "10.0.0.1:80"` | 同一個服務裡，每個 backend 只能出現一次。 |
| `weight must be from 1 to 100` | |
| `at least one backend must not be a backup` | |
| ``unknown checker `nope` `` | `check` 寫的既不是內建檢查，也不是有宣告的 plugin。 |
| `check_options.bogus: unknown key` | 拼錯了，或寫成了別種檢查的選項。 |
| `path: must start with '/'` | 指的是 http 的 `path`。 |
| `expect_status[0]: status 700 outside 100-599` | |
| `tls_verify: requires tls = true` | tcp 的 TLS 相關 key 要搭配 `tls = true`。 |
| `expect: required when send is set` | tcp。 |
| `command: "check.sh" is not an absolute path` | exec 和 plugin 的指令要寫絕對路徑。 |
| `env_pass: EMFOR_IP is set by emfor` | 不要把 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE` 列進去。 |
| `site: invalid site label: 1-63 characters of [a-z0-9-], not starting or ending with -` | |
| `/etc/emfor/conf.d/60-s.toml: store is only allowed in the main config file` | `[store]` 和 `include_dir` 只能寫在主設定檔。 |
| `/etc/emfor/conf.d/70-b.toml:1:24: invalid TOML: unclosed table, expected ']'` | TOML 語法錯誤；訊息裡有行號和欄位。 |

## reload 沒有生效

reload 只有全部套用或全部不套用兩種結果。log 會長這樣：

```text
ERROR emfor_daemon::cli: invalid config /etc/emfor/emfor.toml: 2 error(s)
...
reload rejected; keeping the previous config
```

有些設定永遠不會透過 reload 生效：
`store: store settings require restart`、
`dns.listen: dns listen settings require restart`，`metrics.listen` 和 `peers.listen` 也一樣。改這些設定要重新啟動。

## 每次查詢都失敗

| 錯誤 | 原因 | 處理 |
| --- | --- | --- |
| `UNAVAILABLE: store /var/lib/emfor/state.mdb does not exist; is the emfor daemon running?` | daemon 從來沒跑過，或 `EMFOR_STORE` 指到別的地方。 | 啟動 daemon；對一下 `[store] path` 和 `EMFOR_STORE` 是否一致。 |
| `UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied` | 應用程式的使用者讀不到 store，或寫不了 lock 檔。 | 把這個使用者加進 daemon 的群組（[第 12 章](12-operating.md#誰能讀取-store)），再重新啟動應用程式。 |
| `UNAVAILABLE: EMFOR_MAX_STALENESS="3" is invalid: ...` | 這個變數要加單位。 | `EMFOR_MAX_STALENESS=3s`。 |
| `UNAVAILABLE`，訊息說 LMDB 格式不同 | client 和 daemon 不是同一個發行版。 | 安裝和 daemon 同一個發行版的 client。 |
| `INCOMPATIBLE` | client 比 store 格式還舊（0.4 以前的 client 碰到版本 2 的 store），或有 client 解析不了的紀錄（資料損壞，或是值得回報的 bug）。 | 升級 client；如果 client 已經和 daemon 是同一個發行版，請回報。 |
| `NOT_FOUND: no service named "nope.svc.emfor"` | 拼錯了，或這台主機的設定裡沒有這個服務。 | 用 `emfor status` 看有哪些服務。 |

在命令列上，`emfor status` 也會顯示同樣的問題：
`cannot open store /var/lib/emfor/state.mdb: no such file` 和
`cannot open store ...: Permission denied (os error 13)`（結束代碼 2）。

## 某個服務 ALL DOWN

```text
ALL_DOWN: service db.svc.emfor: no backend is healthy, and on_all_down is closed
```

1. `emfor status --service db.svc.emfor` 會列出每個 backend 的 reason。
2. `emfor lookup db.svc.emfor --explain` 會說明每個 backend 為什麼被排除；所有 backend 都是 `drained` 時，一樣會得到 `ALL_DOWN`。
3. 如果訊息寫的是
   `the store is stale (emfor daemon not updating it)`，那問題不在 backend，而是在 daemon。這時 `emfor status` 會顯示
   `daemon: stale, heartbeat 4.5s ago` 或 `daemon: stopped`。
4. 在同一台主機上手動跑一次檢查（`curl`、`nc`，或用 daemon 的使用者跑 exec 程式）。

## 解讀 reason

| Reason | 來自 | 意義 |
| --- | --- | --- |
| `Connection refused (os error 111)` | tcp | 那個 port 上沒有程式在聽。 |
| `timeout` | 任何檢查 | 在 `timeout` 時間內沒有回應。 |
| `expect: got "..."` | tcp | 回應和 `expect` 對不上。 |
| `error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)` | http | 沒有程式在聽。 |
| `unexpected status 503` | http | 狀態碼不在 `expect_status` 裡。 |
| `cannot build http client: ...; https checks need the system CA certificates unless tls_verify = false` | http（error） | 安裝 `ca-certificates`，或設 `tls_verify = false`。 |
| `exit code 1`，或程式輸出的第一行 | exec | 檢查程式判定 down。 |
| `cannot run /path: ...` | exec（error） | 程式找不到、不能執行，或 unit 的沙箱不允許。 |
| `killed by signal 9` | exec | 程式當掉了，或被 kill 掉。 |
| `connect: Connection refused (os error 111)` | emfor-check plugin | |
| `plugin <name> unavailable`、`plugin exited` | plugin（error） | plugin 當掉了，或正在重新啟動；請看 log。 |
| `plugin <name> busy: it is not reading its requests` | plugin（error） | plugin 不再讀取請求；連續 3 個請求沒回應，它就會被重新啟動。 |
| `password_file: ...` | plugin（error） | daemon 的使用者讀不到這個檔案。 |
| `peers 2/3: <reason>` | 聯防群組 | 三台負責主機中有兩台認為它 down。 |
| `local: <reason>` | 聯防群組 | 這台主機認為它 down，但投票結果不是 down。 |
| `drained` | store | 設了 `drain = true`（[第 8 章](08-choosing-a-backend.md#drain)）。 |
| `check task restarted`（出現在狀態歷史裡） | daemon | 某個檢查 task 停掉後又被重新啟動。請在 log 裡找 `check task of backend ... stopped`，並回報。 |

`error` 絕不會把 backend 標成 down；但如果在 `fall × interval` 這段時間內只有 error，backend 會回到 `unknown`。

## backend 反覆變化（flapping）

在 `emfor status` 裡看到 `FLAPPING (5 changes in 10m)`，或收到 `EmforBackendFlapping` 告警，就是狀態變化太頻繁。通常是 `timeout` 太接近 backend 平常的回應時間，或 backend 負載過重。用 `emfor status --history --service <name>` 可以看到什麼時候變、為什麼變。可以把 `timeout` 調高（最多到 `interval`）、把 `fall` 和 `rise` 調高，或是把 backend 修好。

## 手動檢查可以，在 emfor 裡卻不行

- **環境變數。** 從 0.7.0 起，檢查程式只拿得到 `PATH`、`LANG`、`TZ` 和 `LC_*`。程式需要的其他變數，請加進 `env_pass`。
- **使用者。** daemon 是用 `emfor` 身分執行的；請用 `runuser -u emfor -- ...` 來跑檢查。
- **沙箱。** unit 會讓檔案系統變成唯讀，也會隱藏其他行程；會寫檔或讀 `/home` 的檢查，在那裡會失敗。
- **重新導向。** http 檢查不會跟隨重新導向，而 `301` 落在預設的 `200-399` 裡，所以重新導向會算成 up。要強制這一點，就設 `expect_status = ["200"]`（或把 `path` 直接指到最終頁面）。
- **proxy。** http 檢查會忽略 `http_proxy`。

## DNS 無法解析

| 症狀 | 原因 | 處理 |
| --- | --- | --- |
| 完全沒有回答 | 沒有 `[dns]` table、daemon 停了，或 port 不對。 | `dig @127.0.0.1 -p 5300 <service>`。 |
| `REFUSED` | 名稱不在 zone 裡。 | 檢查 `[dns] zone`。 |
| `NXDOMAIN` | 沒有這個服務，或服務剛加進來（否定快取會維持 `ttl` 這麼久）。 | `emfor status`。 |
| `SERVFAIL` | 全部 down 且設成 `closed`、每個 backend 都被 drain、daemon 剛啟動，或是有做驗證的 unbound 沒設 `domain-insecure`。 | `emfor lookup <service> --explain`。 |
| `dig @127.0.0.1 -p 5300` 查得到，`getent hosts` 查不到 | 主機的 resolver 沒有轉送這個 zone。 | [第 10 章](10-dns.md#從主機的-resolver-轉送)。 |
| 已經故障的 backend 還是被回傳 | resolver 或 runtime 有快取。 | [第 10 章](10-dns.md#快取)。 |

## daemon 在執行，store 卻看起來過期

- `emfor_store_commit_failures_total` 一直增加，log 裡出現 `MDB_MAP_FULL`：代表 store 滿了。調高 `[store] map_size` 再重新啟動。
- 主機的時鐘跳動了：client 是拿自己的時鐘去和 heartbeat 比的。
- daemon 負載過重，或被暫停了（`SIGSTOP`、除錯器）。

## lock 檔屬於其他使用者

如果 `<store>-lock` 在應用程式還在跑的時候被刪掉，第一個開啟 store 的行程會重建這個檔案，擁有者就變成它自己，之後 daemon 可能就啟動不了。這時請先停掉應用程式，或是對 lock 檔執行 `chown emfor:emfor` 和 `chmod 0660`，再啟動 daemon。千萬不要只刪 lock 檔。

## 聯防群組的問題

| 症狀 | 原因 |
| --- | --- |
| `emfor_peer_messages_rejected_total{reason="auth"}` 上升 | 成員之間的密鑰不同，或有一台在換密鑰的過程中重新啟動了（[第 11 章](11-peer-groups.md#更換途中重新啟動)）。 |
| `reason="plaintext"` | 有成員送出 `EMP1`（`encrypt = false`），對方卻不收這種訊息。 |
| `reason="sender"` | 訊息來自不在 `members` 裡的位址（NAT、`listen` 設錯）。 |
| `reason="site"` | 有成員的 `site` 不一樣。 |
| `reason="stale"` 或 `replay` | 時鐘不一致，或訊息在傳送途中被複製。 |
| `peers: 1/2 live, ... too few live members to vote` | 存活成員不到三台：不投票，每台各自判斷。 |
| `emfor status --json` 裡的 digest 不符 | 成員檢查服務的方式不一樣（選項、timeout、plugin 指令、`env_pass`）。 |
| `emfor status` 裡出現 `local-up` | 投票結果是 down，這台卻是 up：只有這台主機才有的網路路徑，或檢查方式不同。 |
