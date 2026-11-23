# 13. 安全模型

[手冊](README.md) · 上一章：[12. 維運 emfor](12-operating.md) · 下一章：[14. 整合](14-integrations.md)

這一章說明 emfor 信任什麼、保護什麼，還有哪些要你自己顧好。內容以 [SECURITY.md](../../SECURITY.md) 和 0.7.0 的程式碼為準。

## 信任什麼、不信任什麼

| 對象 | 是否信任 | 為什麼重要 |
| --- | --- | --- |
| 設定檔與 `include_dir` | 是 | 裡面指定了 daemon 要執行的程式（`exec` 指令、plugin）。 |
| plugin 與 `exec` 程式 | 是 | 它們用 daemon 的權限執行。 |
| backend | **否** | backend 或 plugin 送來的任何東西，進 store、log、status 或 metrics 之前，都會先限制長度再跳脫。 |
| peer | 只在持有密鑰時 | 每則訊息都用共用密鑰驗證（預設也會加密），還會檢查是不是重送、寄件者和 site 對不對。 |
| 本機使用者 | 部分 | 讀 store 的程式共用同一個 lock 檔（[見下](#store-與本機使用者)）。 |
| 到 DNS 與 `/metrics` 的網路 | 沒有認證；`/metrics` 可用 `[metrics] allow` 限制來源 | 兩者預設都只聽 loopback。`allow` 只看直接連進來的對端位址（不讀 `X-Forwarded-For`），不能取代防火牆。 |

## 設定檔

遇到下列情況，daemon 和 `emfor check-config` 會拒絕載入主設定檔、`include_dir` 裡的每個檔案，以及這個目錄本身：

- group 或其他人可以寫入：
  `/etc/emfor/emfor.toml: writable by group or others (mode 0664); refusing to load it; to fix it: chmod g-w,o-w /etc/emfor/emfor.toml`；
- 擁有者不是 root，也不是執行 daemon 的使用者：
  `... owned by uid 1001, not root or the user running emfor (uid 998); refusing to load it; to fix it: chown root ...`。

0.7.0 以前只檢查 `include_dir` 裡的檔案。這些檔案請維持 `root:root 0644`；如果裡面有不想讓其他使用者看到的內容，就用 `0640` 加群組 `emfor`。密碼一律不寫進設定檔，要放在 `password_file` 裡。

要知道設定什麼時候改了、改成什麼，請看 log 裡的稽核行（[第 12 章](12-operating.md#reload-改了什麼)）：每個讀到的檔案都有 SHA-256，每次 reload 都會列出差異。它們不會說是誰改的；那要用 auditd 監看設定目錄。

## 祕密

| 檔案 | 規則 | 警告 |
| --- | --- | --- |
| `[peers] secret_file`、`previous_secret_file` | 絕對路徑；擁有者是 root 或 daemon 的使用者；其他人不能讀；group 與其他人不能寫；大小 32 bytes 到 64 KiB。不符合就是錯誤。 | |
| `emfor-check` 的 `password_file`、`secret_file`（radius）、`tls_key_file`（diameter） | 絕對路徑；不能是空檔。 | 所有使用者都能讀，或其他人能改時，會發出警告。 |
| 不經 TLS 送出的密碼（redis、ldap、mysql、pgsql、amqp） | | 發出警告（[第 7 章](07-plugins.md#密碼)）。 |

建議這樣做：

```sh
# install -d -m 0750 -o root -g emfor /etc/emfor/secrets
# install -m 0640 -o root -g emfor /dev/null /etc/emfor/secrets/pg.pass
```

紀錄檔 `<store>-peers` 只存 peer 密鑰的 SHA-256 指紋，從來不存密鑰本身。

## 檢查程式拿到什麼

`exec` 檢查與 plugin：

- 從 daemon 的環境變數裡只拿得到 `PATH`、`LANG`、`TZ`、`LC_*` 和 `env_pass` 列出的名稱（`exec` 另外還有 `EMFOR_IP`、`EMFOR_PORT`、`EMFOR_SERVICE`）。daemon 環境裡的祕密，你沒列出來就不會傳給它們。0.6 和更早的版本會全部傳過去；
- 在自己的 process group 裡執行，檢查結束或 plugin 停止時，整個 group 會被 kill；
- 用 daemon 的使用者身分、在同一個 systemd 沙箱裡執行。

可以的話，用 daemon 的使用者執行 `emfor check-config`，不要用 root：它會啟動每個 plugin 來驗證選項，用 root 跑的話，這些 plugin 就會跑在沙箱外面。`examples/deploy/` 裡的範例腳本就是這樣做的。

## 來自 backend 的不可信輸入

backend 回的任何內容都可能出現在 reason 裡。所以 emfor 會：

- 把 reason 截到 512 個字元（結尾加 `…`），並跳脫控制字元，backend 就沒辦法偷加 log 行，也沒辦法對 `emfor status` 送終端機跳脫序列；
- `expect`（tcp）最多讀 16384 bytes，`expect_body`（http）最多讀 65536 bytes；plugin 每一行最多 65536 bytes；plugin 標準錯誤輸出的每一行最多只記 1000 個字元；
- 不跟隨 HTTP 重新導向，也不理會 proxy 變數。

從 peer 收到的 reason，記進 log 之前會截到 256 個字元並跳脫。

## store 與本機使用者

LMDB 的 reader 要在 lock 檔裡登記，所以每個應用程式的使用者都必須能**寫入** `<store>-lock`。能寫這個檔的人，也能讓 daemon 的 commit 卡住，或干擾其他 reader。預設權限（0640 與 0660，擁有者和群組都是 daemon 的）讓只有 daemon 群組的成員做得到。設成 `lock_file_mode = "0666"` 的話，每個本機使用者都做得到，daemon 也會發出警告：

```text
[store] lock_file_mode = "0666" lets every user on this host write the lock file, ...
Use "0660" (the default) and put the users of your applications in the group of the emfor daemon.
```

所以 `emfor` 群組的成員可以影響這台主機上的查詢。這個群組只加應用程式的使用者。

store 裡存的是服務名稱、backend 位址、狀態和 reason，不存任何祕密。

## DNS 與 metrics

兩者都沒有認證，預設都只聽 `127.0.0.1`；DNS 聽在其他位址時，daemon 會發出警告；`/metrics` 聽在其他位址又沒設 `[metrics] allow` 時也會。`allow` 列出可以讀 `/metrics` 的網段，其他來源得到 `403`。如果要對外開放，請用防火牆限制只讓需要的主機連進來。DNS server 對 UDP 有依網段計算的速率限制（[第 10 章](10-dns.md#速率限制)），回答的大小也絕不會超過查詢端宣告可接受的大小。

## 聯防訊息

- 用 HMAC-SHA256 驗證（`EMP1`），或用 ChaCha20-Poly1305（`EMP2`）或 XChaCha20-Poly1305（`EMP3`）加密；金鑰是用 HKDF-SHA256 從密鑰導出來的。
- 從 0.7.0 起，`encrypt = true` 的 daemon 預設會丟掉明文訊息（`accept_plaintext = false`）。
- 重送的訊息、太舊的訊息、不認識的寄件者，以及其他 site 來的訊息，都會被丟掉並計數。
- 單一 peer 沒辦法自己改變 backend 的狀態：`down` 通知只會讓本機重新檢查，而投票要三台負責主機中的兩台同意。不過，拿到密鑰的人可以靠送投票，讓每個成員都把某個 backend 標成 down。請像保護密碼一樣保護密鑰。

## systemd unit

套件附的 unit 用 `emfor` 身分執行 daemon，不給任何 capability，除了 store 目錄以外整個檔案系統都是唯讀，另外還設了 `NoNewPrivileges`、system call 過濾、`UMask=0027`、`ProtectProc=invisible` 等等（[第 4 章](04-installing.md#systemd-unit)）。用 `systemd-analyze security emfor` 可以看結果；寫這份手冊時沒有實際跑過。

## SELinux 與 AppArmor

emfor 不附 SELinux 模組，也不附 AppArmor profile，而且沒有在 SELinux enforcing 或 AppArmor profile 底下跑過：測試是在 Debian container 裡跑的，那裡兩者都沒開。底下寫的是程式實際會做什麼，讓你自己寫 policy；它不是一份測過的 policy。

上面的 systemd unit 從 daemon 這一側限制它能做什麼，強制存取控制（MAC）的 policy 則從 kernel 這一側限制，兩者疊加，不能互相取代。稽核規則如果要求 daemon 要被限制住，就需要 policy。

daemon 會碰到的東西：

- **讀：** 設定檔和 `include_dir` 裡的檔案（[第 5 章](05-configuring.md)）、祕密檔、檢查裡指定的 `ca_file`、`password_file` 和金鑰檔，以及系統的 CA 憑證（沒設 `ca_file` 的 `http` 檢查）。它也用 inotify 監看聯防祕密檔所在的目錄。
- **讀寫：** store 目錄，預設是 `/var/lib/emfor`：`state.mdb`、`state.mdb-lock`、`state.mdb.writer`、`state.mdb-peers`，以及 store 損壞時被移開的 `state.mdb.corrupt`。它會在那裡建立檔案並 rename。
- **監聽：** DNS 位址（UDP 與 TCP）、metrics 位址（TCP）、`[peers]` 位址（UDP），都由設定檔指定。
- **連線：** 每個 backend 位址，用檢查的協定（TCP；有些外掛用 UDP）。
- **啟動程式：** `exec` 檢查和外掛會以 `emfor` 身分、在各自的 process group 裡啟動設定檔寫的程式。外掛通常是 `/usr/bin/emfor-check`。要限制 daemon 的 policy 必須允許你寫在設定檔裡的每一支程式，而這些程式也受同一套規則限制；這通常是需要你自己加規則的地方。
- **不會用到：** raw socket、packet socket 或 capability；正常執行時也不讀 `/proc`。

如果你的發行版只在載入 policy 時才限制 daemon，那沒有 policy 的 daemon 就不會被限制；你的系統怎麼處理沒有標籤的程式，我們沒有查證。加了 policy 之後遇到拒絕的處理，見[第 16 章](16-troubleshooting.md#selinux-或-apparmor-的拒絕)。

## 回報安全漏洞

請用 GitHub repository 上的私人漏洞回報功能（先按 Security，再按 Report a vulnerability），不要開公開的 issue。1.0 以前，只有最新的發行版會收到安全修正。
