# emfor 使用手冊

這份手冊寫給從來沒用過 emfor 的人：一種是負責安裝、執行 emfor 的管理員，另一種是開發者，他們的應用程式要問 emfor 該連哪個 backend。本手冊的內容以 **emfor 0.8.0** 為準。

先讀第 1 章，看看 emfor 適不適合解決你的問題，再照第 2 章的快速上手做一遍。在一台 Linux 主機上大約十分鐘就能做完。之後需要哪一章再讀哪一章。第 18 章用一張表列出所有設定 key。

## 章節

| # | 章節 | 什麼時候讀 |
| --- | --- | --- |
| 1 | [emfor 是什麼，不是什麼](01-what-emfor-is.md) | 想判斷 emfor 適不適合 |
| 2 | [快速上手](02-quick-start.md) | 想花十分鐘在一台主機上看它跑起來 |
| 3 | [基本概念](03-concepts.md) | 想了解服務、backend、健康狀態、層級與 store |
| 4 | [安裝](04-installing.md) | 要安裝 daemon、套件、服務 unit 與 client 函式庫 |
| 5 | [設定 emfor](05-configuring.md) | 要規劃設定檔、檢查設定、reload |
| 6 | [健康檢查：tcp、http 與 exec](06-health-checks.md) | 要挑選並調整內建的檢查 |
| 7 | [Plugin 與 emfor-check](07-plugins.md) | 要檢查資料庫、LDAP、DNS、RADIUS、SIP、Diameter、SMTP、AMQP、MongoDB、gRPC 或 QUIC，或想自己寫檢查 |
| 8 | [如何選出 backend](08-choosing-a-backend.md) | 要挑 `balance` 方式，或使用備援、drain 與慢啟動 |
| 9 | [Client 函式庫](09-clients.md) | 要從 Rust、C、Go、Python、Perl 或 PHP 查詢 backend |
| 10 | [DNS](10-dns.md) | 要服務那些沒辦法用 client 函式庫的程式 |
| 11 | [聯防群組](11-peer-groups.md) | 想讓同一個資料中心的主機分享檢查結果 |
| 12 | [維運 emfor](12-operating.md) | 要用 `emfor status` 與 `emfor lookup`、reload、看 metrics 與 log、照顧 store |
| 13 | [安全模型](13-security.md) | 想知道誰能做什麼，以及哪些東西要鎖好 |
| 14 | [整合](14-integrations.md) | 要把資料餵給 nginx、HAProxy、Caddy、Prometheus 或 pf，或用 Nagios 監看 emfor |
| 15 | [升級](15-upgrading.md) | 要從 0.7（或更舊的版本）升級到 0.8.0 |
| 16 | [疑難排解](16-troubleshooting.md) | 要從錯誤訊息著手修好問題 |
| 17 | [從 HAProxy 或 Consul 搬過來](17-migrating.md) | 要轉換現有的設定 |
| 18 | [設定參考](18-config-reference.md) | 要查某個 key、它的預設值與限制 |
| 19 | [詞彙表](19-glossary.md) | 要查某個詞 |

## 慣例

- `$` 開頭的是你用一般使用者輸入的指令，`#` 開頭的是用 root 輸入的指令。兩個都沒有的是輸出。
- 路徑都是預設值：設定檔在 `/etc/emfor/emfor.toml`，store 在 `/var/lib/emfor/state.mdb`，plugin 程式在 `/usr/bin/emfor-check`（deb 與 rpm 套件就裝在這裡）。
- 服務名稱都以 `.svc.emfor` 結尾，這是預設的 DNS zone。名稱只要長得像主機名稱就能用，但只有 DNS zone 底下的名稱才能透過 DNS 查到。
- 範例用的是 `10.0.0.11` 這類私有位址。快速上手用 `127.0.0.1` 與 `127.0.0.2`，這樣在一台機器上就能跑。

## 本手冊中「測試過」的意思

本手冊裡每個完整的 TOML 範例，都在 Debian 13 container 裡用 0.7.0 的 `emfor check-config` 檢查過。書中的指令輸出也是 emfor 0.7.0 在這種 container 裡產生的，內文另有說明的除外；0.8.0 新增的部分會說明用什麼跑過，`emfor --version` 的輸出則是 0.8.0 的。有些說法來自 emfor 自己的測試（例如 nginx 與 HAProxy 的做法），內文會註明。沒有實際跑過的東西（macOS、amd64 上的 FreeBSD、實體硬體、真正的資料庫伺服器），內文會在重要的地方說明。

repository 裡有更詳細的參考文件，相關的地方會附上連結：daemon、DNS 與 plugin 的細節在 [docs/deploy/](../deploy/)，plugin 協定在 [docs/plugins/protocol-v1.md](../plugins/protocol-v1.md)，各 client 的 README 在 [clients/](../../clients/) 底下。
