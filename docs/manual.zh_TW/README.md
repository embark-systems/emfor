# emfor 使用手冊

這份手冊寫給從來沒用過 emfor 的人：安裝並執行它的管理員，以及應用程式要向它詢問該連哪個 backend 的開發者。本手冊描述 **emfor 0.7.0**。

先讀第 1 章，看 emfor 是否適合你的問題，接著照第 2 章的快速上手做一次。在一台 Linux 主機上大約要十分鐘。之後再讀你需要的章節。第 18 章把每個設定 key 列在一張表裡。

## 章節

| # | 章節 | 什麼時候讀 |
| --- | --- | --- |
| 1 | [emfor 是什麼，不是什麼](01-what-emfor-is.md) | 決定 emfor 是否適合 |
| 2 | [快速上手](02-quick-start.md) | 十分鐘內在一台主機上看它運作 |
| 3 | [基本概念](03-concepts.md) | 理解服務、backend、健康狀態、層級與 store |
| 4 | [安裝](04-installing.md) | 安裝 daemon、套件、服務 unit 與 client 函式庫 |
| 5 | [設定 emfor](05-configuring.md) | 安排設定檔、檢查它們並 reload |
| 6 | [健康檢查：tcp、http 與 exec](06-health-checks.md) | 選擇並調整內建的檢查 |
| 7 | [Plugin 與 emfor-check](07-plugins.md) | 檢查資料庫、LDAP、DNS、RADIUS、SIP、Diameter、SMTP、AMQP、MongoDB、gRPC 或 QUIC，或自己寫檢查 |
| 8 | [如何選出 backend](08-choosing-a-backend.md) | 挑選 `balance` 方式，並使用備援、drain 與慢啟動 |
| 9 | [Client 函式庫](09-clients.md) | 從 Rust、C、Go、Python、Perl 或 PHP 查詢 backend |
| 10 | [DNS](10-dns.md) | 服務無法使用 client 函式庫的程式 |
| 11 | [聯防群組](11-peer-groups.md) | 讓同一個資料中心的主機分享檢查結果 |
| 12 | [維運 emfor](12-operating.md) | 使用 `emfor status` 與 `emfor lookup`、reload、讀 metrics 與 log、照顧 store |
| 13 | [安全模型](13-security.md) | 知道誰能做什麼，以及要鎖住哪些東西 |
| 14 | [整合](14-integrations.md) | 提供資料給 nginx、HAProxy、Caddy、Prometheus 或 pf，或從 Nagios 監看 emfor |
| 15 | [升級](15-upgrading.md) | 從 0.6（或更舊）升級到 0.7.0 |
| 16 | [疑難排解](16-troubleshooting.md) | 從錯誤訊息開始修好問題 |
| 17 | [從 HAProxy 或 Consul 搬過來](17-migrating.md) | 轉換既有的設定 |
| 18 | [設定參考](18-config-reference.md) | 查一個 key、它的預設值與限制 |
| 19 | [詞彙表](19-glossary.md) | 查一個詞 |

## 慣例

- `$` 開頭的是你以一般使用者輸入的指令，`#` 開頭的是以 root 輸入的指令。兩者都沒有的是輸出。
- 路徑都是預設值：設定檔在 `/etc/emfor/emfor.toml`，store 在 `/var/lib/emfor/state.mdb`，plugin 程式在 `/usr/bin/emfor-check`（deb 與 rpm 套件放的位置）。
- 服務名稱都以 `.svc.emfor` 結尾，也就是預設的 DNS zone。任何類似主機名稱的名字都可以用，但只有 DNS zone 之下的名稱能透過 DNS 解析。
- 範例使用 `10.0.0.11` 這類私有位址。快速上手使用 `127.0.0.1` 與 `127.0.0.2`，這樣在一台機器上就能執行。

## 本手冊中「測試過」的意思

本手冊中每個完整的 TOML 範例，都在 Debian 13 container 中以 0.7.0 的 `emfor check-config` 檢查過。顯示的指令輸出由 emfor 0.7.0 在這樣的 container 中產生，內文另有說明者除外。有些敘述來自 emfor 自己的測試（例如 nginx 與 HAProxy 的做法），內文會這樣說明。沒有實際跑過的東西（macOS、amd64 上的 FreeBSD、實體硬體、真正的資料庫伺服器），內文在重要之處會說明。

repository 中較詳細的參考文件會在相關之處附上連結：[docs/deploy/](../deploy/) 有 daemon、DNS 與 plugin 的細節，[docs/plugins/protocol-v1.md](../plugins/protocol-v1.md) 是 plugin 協定，各 client 的 README 在 [clients/](../../clients/) 底下。
