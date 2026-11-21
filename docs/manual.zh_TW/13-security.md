# 13. 安全模型

[手冊](README.md) · 上一章：[12. 維運 emfor](12-operating.md) · 下一章：[14. 整合](14-integrations.md)

這一章說明 emfor 信任什麼、保護什麼，以及哪些要你自己保護。內容依照 [SECURITY.md](../../SECURITY.md) 與 0.7.0 的程式碼。

## 信任什麼、不信任什麼

| 對象 | 是否信任 | 為什麼重要 |
| --- | --- | --- |
| 設定檔與 `include_dir` | 是 | 它們指定 daemon 要執行的程式（`exec` 指令、plugin）。 |
| plugin 與 `exec` 程式 | 是 | 它們以 daemon 的權限執行。 |
| backend | **否** | backend 或 plugin 送來的任何東西，在進入 store、log、status 或 metrics 之前，都會先限制長度並跳脫。 |
| peer | 只在持有密鑰時 | 每則訊息都以共用密鑰驗證（預設也加密），並檢查是否為重送、寄件者與 site。 |
| 本機使用者 | 部分 | store 的讀取者共用它的 lock 檔（[見下](#store-與本機使用者)）。 |
| 到 DNS 與 `/metrics` 的網路 | 沒有認證 | 兩者預設都聽在 loopback。 |

## 設定檔

daemon 與 `emfor check-config` 在下列情況拒絕載入主設定檔、`include_dir` 裡的每個檔案以及該目錄：

- group 或其他人可以寫入：
  `/etc/emfor/emfor.toml: writable by group or others (mode 0664); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/emfor.toml`；
- 擁有者不是 root，也不是 daemon 自己的使用者：
  `... owned by uid 1001, not root or the user running emfor (uid 998); refusing to load it; to fix it: chown root ...`。

0.7.0 之前只有 `include_dir` 的檔案會這樣檢查。請讓這些檔案保持 `root:root 0644`（如果裡面寫了你不想讓其他使用者看到的東西，用 `0640` 加上群組 `emfor`）。密碼絕不寫進設定檔：密碼放在 `password_file` 裡。

## 祕密

| 檔案 | 規則 | 警告 |
| --- | --- | --- |
| `[peers] secret_file`、`previous_secret_file` | 絕對路徑；擁有者是 root 或 daemon 的使用者；其他人不能讀取；group 與其他人不能寫入；32 bytes 到 64 KiB。否則是錯誤。 | |
| `emfor-check` 的 `password_file`、`secret_file`（radius）、`tls_key_file`（diameter） | 絕對路徑；不能是空的。 | 每個使用者都能讀，或其他人能修改時發出警告。 |
| 不經 TLS 送出的密碼（redis、ldap、mysql、pgsql、amqp） | | 發出警告（[第 7 章](07-plugins.md#密碼)）。 |

建議：

```sh
# install -d -m 0750 -o root -g emfor /etc/emfor/secrets
# install -m 0640 -o root -g emfor /dev/null /etc/emfor/secrets/pg.pass
```

紀錄檔 `<store>-peers` 存的是 peer 密鑰的 SHA-256 指紋，絕不存密鑰本身。

## 檢查程式拿到什麼

`exec` 檢查與 plugin：

- 從 daemon 的環境只拿到 `PATH`、`LANG`、`TZ`、`LC_*` 與 `env_pass` 列出的名稱（`exec` 另外還有 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`）。daemon 環境裡的祕密，除非你列出來，否則不會到它們手上。0.6 與更早的版本會拿到全部；
- 在自己的 process group 中執行，檢查結束或 plugin 停止時整個 group 會被 kill；
- 以 daemon 的使用者、在同一個 systemd 沙箱下執行。

可以的話，以 daemon 的使用者而不是 root 執行 `emfor check-config`：它會啟動每個 plugin 來驗證選項，以 root 執行時它們會在沙箱之外執行。`examples/deploy/` 裡的範例腳本就是這樣做的。

## 來自 backend 的不可信輸入

backend 說的任何話都可能進到 reason 裡。emfor：

- 把 reason 截在 512 個字元（以 `…` 結尾），並跳脫控制字元，所以 backend 無法多加 log 行，也無法對 `emfor status` 送出終端機跳脫序列；
- `expect`（tcp）最多讀 16384 bytes，`expect_body`（http）最多讀 65536 bytes，plugin 的每一行限制在 65536 bytes，plugin 標準錯誤輸出的每一行最多記錄 1000 個字元；
- 不跟隨 HTTP 重新導向，並忽略 proxy 變數。

從 peer 收到的 reason 在記錄之前會截到 256 個字元並跳脫。

## store 與本機使用者

LMDB 的 reader 會在 lock 檔登記，所以每個應用程式使用者都必須能**寫入** `<store>-lock`。能寫它的人也能讓 daemon 的 commit 停住，或干擾其他 reader。預設值（0640 與 0660，擁有者與群組是 daemon 的）把這限制在 daemon 的群組。設定 `lock_file_mode = "0666"` 時就是每個本機使用者，daemon 會發出警告：

```text
[store] lock_file_mode = "0666" lets every user on this host write the lock file, ...
Use "0660" (the default) and put the users of your applications in the group of the emfor daemon.
```

因此 `emfor` 群組的成員能影響這台主機上的查詢。只把應用程式的使用者加進這個群組。

store 存的是服務名稱、backend 位址、狀態與 reason，沒有任何祕密。

## DNS 與 metrics

兩者都沒有認證。兩者預設都聽在 `127.0.0.1`；DNS 聽在其他位址時 daemon 會發出警告。如果要對外開放，請用防火牆限制只有需要的主機能連。DNS server 對 UDP 有以網段計算的速率限制（[第 10 章](10-dns.md#速率限制)），回答也絕不超過查詢所提供的大小。

## 聯防訊息

- 以 HMAC-SHA256 驗證（`EMP1`），或以 ChaCha20-Poly1305（`EMP2`）或 XChaCha20-Poly1305（`EMP3`）加密，金鑰以 HKDF-SHA256 從密鑰導出。
- 從 0.7.0 起，`encrypt = true` 的 daemon 預設丟棄明文訊息（`accept_plaintext = false`）。
- 重送的訊息、舊訊息、不認識的寄件者與其他 site 的訊息會被丟棄並計數。
- peer 無法單獨改變 backend 的狀態：`down` 通知只會觸發本機重新檢查，而投票需要三台負責主機中的兩台。不過持有密鑰的人可以藉由送出投票，讓每個成員都把某個 backend 標成 down。請像保護密碼一樣保護密鑰。

## systemd unit

套件附的 unit 以 `emfor` 執行 daemon，不帶任何 capability，除了 store 目錄之外檔案系統都是唯讀，並設定 `NoNewPrivileges`、system call 過濾、`UMask=0027`、`ProtectProc=invisible` 等（[第 4 章](04-installing.md#systemd-unit)）。`systemd-analyze security emfor` 會顯示結果；這份手冊沒有執行它。

## 回報安全漏洞

請使用 GitHub repository 上的私人漏洞回報功能（Security，再按 Report a vulnerability），不要開公開的 issue。1.0 之前只有最新的發行版會得到安全修正。
