# emfor 使用手冊

[English](../manual/README.md) | 繁體中文

這份手冊寫給安裝、設定 emfor 的人，以及撰寫使用 emfor 之應用程式的人。它說明各個元件如何配合，並逐步帶過每項工作；[docs/deploy/](../deploy/) 底下的文件與各 client 的 README 有完整細節，會在相關之處附上連結（這些文件為英文）。

本手冊描述 emfor 0.4。

| 章節 | 內容 |
| --- | --- |
| [1. 基本概念](01-concepts.md) | daemon、state store、client、DNS、健康狀態，以及出問題時會發生什麼事 |
| [2. 安裝](02-installation.md) | 建置、安裝路徑前綴、systemd、FreeBSD，以及 client 函式庫 |
| [3. 設定](03-configuration.md) | `emfor.toml` 的每個 table 與 key、驗證規則，以及 reload 能改變什麼 |
| [4. 健康檢查](04-health-checks.md) | `tcp`、`http`、`exec`、plugin 與 `emfor-check`；rise、fall、排程與 reason |
| [5. 選擇 backend](05-backend-selection.md) | 各種 `balance` 方式、`on_all_down`，以及過期的 store |
| [6. Client 函式庫](06-clients.md) | 從 Rust、C、Go、Python、Perl、PHP 查詢 backend |
| [7. DNS](07-dns.md) | DNS server、它的回答、SRV 記錄，以及從主機 resolver 轉送 |
| [8. 維運](08-operations.md) | 指令、signal 與 reload、`emfor status`、metrics、log、結束代碼、磁碟上的 store、升級 |
| [9. 疑難排解](09-troubleshooting.md) | 症狀、原因與處理方式 |

第一次接觸 emfor？先讀第 1 章，接著照 [README](../../README.zh-TW.md#快速上手) 的快速上手操作一次，再讀第 3 章。

## 慣例

- 範例中的服務名稱都以 `.svc.emfor` 結尾，也就是預設的 DNS zone。任何類似主機名稱的名字都可以用；只有 zone 之下的名稱能透過 DNS 解析。
- 路徑都是預設值：設定檔在 `/etc/emfor/emfor.toml`，store 在 `/var/lib/emfor/state.mdb`。
- `$` 開頭的是你輸入的指令；沒有 `$` 的是輸出。
- 設定 key、指令、log 與錯誤訊息保留原文，方便對照實際畫面。
