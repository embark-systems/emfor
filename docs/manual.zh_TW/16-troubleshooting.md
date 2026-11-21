# 16. 疑難排解

[手冊](README.md) · 上一章：[15. 升級](15-upgrading.md) · 下一章：[17. 從 HAProxy 或 Consul 搬過來](17-migrating.md)

先執行這三個指令；大部分的問題會出現在其中一個：

```sh
# emfor check-config                       # 設定有效嗎？
# emfor status                             # daemon 在執行嗎？它看到什麼？
# emfor lookup <service> --explain         # 查詢為什麼回答它所回答的？
```

再看 log：`journalctl -u emfor`（systemd），或 tag 為 `emfor` 的 syslog（FreeBSD）。下面的 emfor 訊息是 emfor 0.7.0 在 Debian 測試容器中產生的，但 plugin 的 reason 取自程式碼，systemd 與 DNS 的症狀取自 repository 的文件。

## daemon 無法啟動

| 訊息 | 原因 | 處理 |
| --- | --- | --- |
| `cannot open state store: /var/lib/emfor/state.mdb.writer: No such file or directory (os error 2)` | store 目錄不存在；daemon 不會建立它。 | `install -d -o emfor -g emfor -m 0755 /var/lib/emfor`（unit 的 `StateDirectory=` 會做這件事）。 |
| `cannot open state store: store is already in use by another emfor daemon: lock file /var/lib/emfor/state.mdb.writer is held` | 另一個 daemon 在使用這個 store。 | 停止它，或給這個 daemon 另一個 `[store] path`。 |
| `cannot start metrics server: cannot listen on 127.0.0.1:8080 (tcp): Address already in use (os error 98)` | 這個 port 已被佔用。 | 釋放它，或修改 `listen`。 |
| `invalid config /etc/emfor/emfor.toml: 2 error(s)` 以及各行錯誤 | 設定無效（結束代碼 2）。 | 執行 `emfor check-config`，逐行修正（見下）。 |
| `/etc/emfor/conf.d: writable by group or others (mode 0775); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/conf.d` | 其他人能修改的設定檔或目錄。 | 執行訊息中寫的 `chmod`（或 `chown`）。 |
| `include_dir: cannot read directory /etc/emfor/conf.d: No such file or directory (os error 2)` | `include_dir` 指向不存在的目錄。 | 建立它，或移除 `include_dir`。 |
| `--check-config was replaced by "emfor check-config"` | 舊的命令列。 | 使用 `emfor check-config`。 |
| systemd：重複啟動後 unit 變成 `failed` | 一分鐘內啟動失敗五次（`StartLimitBurst=5`）。 | 排除原因，再執行 `systemctl reset-failed emfor`。 |

## 設定錯誤

`emfor check-config` 每個錯誤印一行，前面是 key 的路徑；若是 `conf.d` 的檔案，後面會接著檔名：

```text
services."web.svc.emfor".timeout: timeout must not exceed interval (in /etc/emfor/conf.d/50-x.toml)
```

| 訊息 | 處理 |
| --- | --- |
| `defaults.interval: invalid duration "2", use e.g. "500ms" or "2s"` | 時間長度需要單位。 |
| `defaults.jitter: jitter above 50% or below 0%` | `jitter` 是 `"0%"`–`"50%"`。 |
| `services.Bad_Name: invalid service name: labels of [a-z0-9-], 1-63 characters each, 253 characters total` | 只能用小寫、DNS 形式的名稱。 |
| `backend "web1:80" must be an IP literal with port: IPv4:port or [IPv6]:port, port 1-65535` | backend 是 IP 位址，絕不是主機名稱。 |
| `duplicate backend "10.0.0.1:80"` | 每個 backend 在一個服務中只能出現一次。 |
| `weight must be from 1 to 100` | |
| `at least one backend must not be a backup` | |
| ``unknown checker `nope` `` | `check` 指定的既不是內建檢查，也不是已宣告的 plugin。 |
| `check_options.bogus: unknown key` | 拼錯了，或是另一種檢查的選項。 |
| `path: must start with '/'` | http 的 `path`。 |
| `expect_status[0]: status 700 outside 100-599` | |
| `tls_verify: requires tls = true` | tcp 的 TLS key 需要 `tls = true`。 |
| `expect: required when send is set` | tcp。 |
| `command: "check.sh" is not an absolute path` | exec 與 plugin 的指令要用絕對路徑。 |
| `env_pass: EMFOR_IP is set by emfor` | 不要列出 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`。 |
| `site: invalid site label: 1-63 characters of [a-z0-9-], not starting or ending with -` | |
| `/etc/emfor/conf.d/60-s.toml: store is only allowed in the main config file` | `[store]` 與 `include_dir` 只能寫在主設定檔。 |
| `/etc/emfor/conf.d/70-b.toml:1:24: invalid TOML: unclosed table, expected ']'` | TOML 語法錯誤；訊息會給出行與欄。 |

## reload 沒有生效

reload 是全有或全無。log 會寫：

```text
ERROR emfor_daemon::cli: invalid config /etc/emfor/emfor.toml: 2 error(s)
...
reload rejected; keeping the previous config
```

有些設定從來不會被 reload：
`store: store settings require restart`、
`dns.listen: dns listen settings require restart`，`metrics.listen` 與 `peers.listen` 也一樣。這些設定要重新啟動。

## 每次查詢都失敗

| 錯誤 | 原因 | 處理 |
| --- | --- | --- |
| `UNAVAILABLE: store /var/lib/emfor/state.mdb does not exist; is the emfor daemon running?` | daemon 從未執行過，或 `EMFOR_STORE` 指向別處。 | 啟動 daemon；比對 `[store] path` 與 `EMFOR_STORE`。 |
| `UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied` | 應用程式的使用者無法讀取 store 或無法寫入 lock 檔。 | 把它加進 daemon 的群組（[第 12 章](12-operating.md#誰能讀取-store)），並重新啟動應用程式。 |
| `UNAVAILABLE: EMFOR_MAX_STALENESS="3" is invalid: ...` | 這個變數需要單位。 | `EMFOR_MAX_STALENESS=3s`。 |
| `UNAVAILABLE`，說 LMDB 格式不同 | client 來自另一個發行版。 | 安裝與 daemon 同一發行版的 client。 |
| `INCOMPATIBLE` | client 比 store 的格式舊（0.4 之前的 client 遇到版本 2 的 store），或 client 無法解析的紀錄（損壞，或值得回報的 bug）。 | 升級 client；如果它已經是和 daemon 同一發行版的，請回報。 |
| `NOT_FOUND: no service named "nope.svc.emfor"` | 拼錯了，或這台主機的設定裡沒有這個服務。 | `emfor status` 會列出服務。 |

`emfor status` 在命令列上顯示同樣的問題：
`cannot open store /var/lib/emfor/state.mdb: no such file` 與
`cannot open store ...: Permission denied (os error 13)`（結束代碼 2）。

## 某個服務 ALL DOWN

```text
ALL_DOWN: service db.svc.emfor: no backend is healthy, and on_all_down is closed
```

1. `emfor status --service db.svc.emfor` 顯示每個 backend 的 reason。
2. `emfor lookup db.svc.emfor --explain` 顯示每個 backend 為什麼被排除；每個 backend 都 `drained` 也會得到 `ALL_DOWN`。
3. 如果訊息寫著
   `the store is stale (emfor daemon not updating it)`，問題不在 backend，而在 daemon。這時 `emfor status` 會顯示
   `daemon: stale, heartbeat 4.5s ago` 或 `daemon: stopped`。
4. 從同一台主機手動執行檢查（`curl`、`nc`，或以 daemon 的使用者執行 exec 程式）。

## 解讀 reason

| Reason | 來自 | 意義 |
| --- | --- | --- |
| `Connection refused (os error 111)` | tcp | 那個 port 上沒有東西在聽。 |
| `timeout` | 任何檢查 | 在 `timeout` 內沒有回答。 |
| `expect: got "..."` | tcp | 回答不符合 `expect`。 |
| `error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)` | http | 沒有東西在聽。 |
| `unexpected status 503` | http | 狀態碼不在 `expect_status` 裡。 |
| `cannot build http client: ...; https checks need the system CA certificates unless tls_verify = false` | http（error） | 安裝 `ca-certificates`，或設定 `tls_verify = false`。 |
| `exit code 1`，或程式輸出的第一行 | exec | 檢查程式說 down。 |
| `cannot run /path: ...` | exec（error） | 找不到、無法執行，或 unit 的沙箱不允許。 |
| `killed by signal 9` | exec | 程式當掉或被 kill。 |
| `connect: Connection refused (os error 111)` | emfor-check plugin | |
| `plugin <name> unavailable`、`plugin exited` | plugin（error） | plugin 當掉或正在重新啟動；看 log。 |
| `plugin <name> busy: it is not reading its requests` | plugin（error） | plugin 不再讀取請求；連續 3 個請求沒有回應後它會被重新啟動。 |
| `password_file: ...` | plugin（error） | daemon 的使用者無法讀取這個檔案。 |
| `peers 2/3: <reason>` | 聯防群組 | 三台負責主機中有兩台看到它 down。 |
| `local: <reason>` | 聯防群組 | 這台主機看到它 down；投票沒有判 down。 |
| `drained` | store | `drain = true`（[第 8 章](08-choosing-a-backend.md#drain)）。 |
| `check task restarted`（在狀態歷史中） | daemon | 某個檢查 task 停止後被重新啟動。在 log 中找 `check task of backend ... stopped` 並回報。 |

`error` 絕不會把 backend 標成 down，但連續 `fall × interval` 都只有 error 時，backend 會回到 `unknown`。

## backend 反覆變化（flapping）

`emfor status` 中的 `FLAPPING (5 changes in 10m)`，或 `EmforBackendFlapping` 告警，表示頻繁的變化。通常是 `timeout` 太接近 backend 平常的回應時間，或 backend 過載。`emfor status --history --service <name>` 會顯示何時、為何變化。調高 `timeout`（最多到 `interval`）、調高 `fall` 與 `rise`，或修好 backend。

## 手動檢查可以，在 emfor 裡卻不行

- **環境。** 從 0.7.0 起，檢查程式只拿到 `PATH`、`LANG`、`TZ` 與 `LC_*`。把它們需要的加進 `env_pass`。
- **使用者。** daemon 以 `emfor` 執行；用 `runuser -u emfor -- ...` 執行檢查。
- **沙箱。** unit 讓檔案系統變成唯讀，並隱藏其他行程；會寫檔或讀取 `/home` 的檢查在那裡會失敗。
- **重新導向。** http 檢查不跟隨重新導向，而 `301` 在預設的 `200-399` 之內，所以重新導向算 up。要要求它，設定 `expect_status = ["200"]`（或把 `path` 指向最終頁面）。
- **proxy。** http 檢查會忽略 `http_proxy`。

## DNS 無法解析

| 症狀 | 原因 | 處理 |
| --- | --- | --- |
| 完全沒有回答 | 沒有 `[dns]` table、daemon 停了，或 port 不對。 | `dig @127.0.0.1 -p 5300 <service>`。 |
| `REFUSED` | 名稱在 zone 之外。 | 檢查 `[dns] zone`。 |
| `NXDOMAIN` | 沒有這個服務，或它剛被加入（否定快取持續 `ttl`）。 | `emfor status`。 |
| `SERVFAIL` | 全部 down 且是 `closed`、每個 backend 都被 drain、daemon 剛啟動，或會驗證的 unbound 沒有設定 `domain-insecure`。 | `emfor lookup <service> --explain`。 |
| `dig @127.0.0.1 -p 5300` 可以，`getent hosts` 不行 | 主機的 resolver 沒有轉送這個 zone。 | [第 10 章](10-dns.md#從主機的-resolver-轉送)。 |
| 故障的 backend 仍被回傳 | resolver 或 runtime 的快取。 | [第 10 章](10-dns.md#快取)。 |

## daemon 在執行，store 卻看起來過期

- `emfor_store_commit_failures_total` 增加，log 中出現 `MDB_MAP_FULL`：store 滿了。調高 `[store] map_size` 並重新啟動。
- 主機的時鐘跳動了：client 用自己的時鐘和 heartbeat 比較。
- daemon 過載或被停住（`SIGSTOP`、除錯器）。

## lock 檔屬於其他使用者

如果 `<store>-lock` 在應用程式執行期間被刪除，第一個開啟 store 的行程會以自己為擁有者重建它，daemon 之後就可能無法啟動。停止應用程式，或對 lock 檔執行 `chown emfor:emfor` 與 `chmod 0660`，再啟動 daemon。絕不要單獨刪除 lock 檔。

## 聯防群組的問題

| 症狀 | 原因 |
| --- | --- |
| `emfor_peer_messages_rejected_total{reason="auth"}` 上升 | 成員的密鑰不同，或有一台在更換密鑰期間重新啟動（[第 11 章](11-peer-groups.md#更換途中重新啟動)）。 |
| `reason="plaintext"` | 某個成員送 `EMP1`（`encrypt = false`）給拒絕它的成員。 |
| `reason="sender"` | 訊息來自不在 `members` 裡的位址（NAT、錯誤的 `listen`）。 |
| `reason="site"` | 某個成員的 `site` 不同。 |
| `reason="stale"` 或 `replay` | 時鐘不同，或訊息在途中被複製。 |
| `peers: 1/2 live, ... too few live members to vote` | 存活成員少於三台：沒有投票，每台各自決定。 |
| `emfor status --json` 中的 digest 不符 | 成員檢查服務的方式不同（選項、timeout、plugin 指令、`env_pass`）。 |
| `emfor status` 中的 `local-up` | 依投票是 down，在這裡卻是 up：只有這台主機有的網路路徑，或不同的檢查。 |
