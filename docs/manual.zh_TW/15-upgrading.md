# 15. 升級

[手冊](README.md) · 上一章：[14. 整合](14-integrations.md) · 下一章：[16. 疑難排解](16-troubleshooting.md)

## 規則

- 所有元件共用同一個版本號，也一起發行：daemon、`emfor-check`、`libemfor`、Rust crate，以及 Go、Python、Perl 與 PHP client。**請從同一個發行版安裝，也一起升級。**`libemfor` 內含 daemon 用的那份 LMDB 建置；版本對不上時會得到 `UNAVAILABLE`，絕不會拿到錯的資料。
- 1.0 以前，minor 版（0.6 → 0.7）可能會改到穩定介面；CHANGELOG 會把這類變更列在 **Read before you upgrade** 底下，或標成 *breaking* / *compatibility change*，並寫明要怎麼處理。patch 版絕不需要你這邊做任何修改。
- store 每次啟動都會重建，所以永遠不需要搬移 store；只有狀態歷史會帶過去。
- 設定檔遇到不認得的 key 會拒絕載入。要降版的話，先拿掉舊版不認得的 key。
- 穩定的部分：設定檔、指令與結束代碼、`status --json`、store 格式、client API、plugin 協定、DNS 回答、metric 名稱。不穩定的部分：`emfor status` 的文字、log 行、錯誤訊息的措辭。寫 script 時請比對代碼和結束代碼。例外有兩個：reload 的稽核行（[第 12 章](12-operating.md#reload-改了什麼)），它的欄位名和 `change` 的值是穩定的；以及狀態變化那一行的欄位，和 JSON log 的 `timestamp`、`level`（[第 12 章](12-operating.md#json-行)）；周圍的文字不是。詳見 [docs/compatibility.md](../compatibility.md)。

## 每台主機上的順序

1. **先升級 client。** 裝好新的 `libemfor` 和包裝函式庫，再重新啟動應用程式讓它們載入新版（執行中的行程會一直用啟動時載入的函式庫）。
2. **用新的 binary 檢查設定**：`emfor check-config`。可以的話用 daemon 的使用者執行。設定無效時，systemd unit 的 `ExecStartPre` 會拒絕啟動，daemon 就會一直停著。
3. **安裝 daemon 並重新啟動。** 用 restart，不要先停止、安裝、再啟動，這樣空窗比較短（[第 12 章](12-operating.md#重新啟動時應用程式會看到什麼)）。
4. **確認**：`emfor status` 的結束代碼是 0，log 裡也沒有意料之外的警告。

一次只處理一台主機。

## 哪些版本會有修正

只有最新版。1.0 之前，bug 修正和安全修正都放進下一個版本，不會回植到舊版，所以要拿到修正就是升級（升級要付出什麼，見上面的規則）。emfor 沒有支援合約，也不承諾回應時間；回報安全漏洞的方式見[第 13 章](13-security.md#回報安全漏洞)。1.0 之後要承諾什麼，還沒有決定。

## 從 0.7 到 0.8

store 格式（版本 2）和 plugin 協定（1）都沒有變。共用函式庫改了名字，所以請照上面的規則，把 0.8 的 client 和 daemon 一起裝。

重新啟動 daemon **之前**，先把這份清單逐項處理完：

| # | 0.8.0 的變更 | 什麼會壞 | 該怎麼做 |
| --- | --- | --- | --- |
| 1 | 共用函式庫是 `libemfor.so.0.8`（原本是 `libemfor.so.0.7`）。 | 連結 `libemfor.so.0.7` 的程式載入不了 0.8。0.8 的 Python、Perl、PHP 和 Go client 找的是新名字。 | 新函式庫裝在舊的旁邊，程式一個一個重新建置或重新安裝（包括 PHP 和 Perl 擴充模組），最後一個換完再拿掉舊檔。 |
| 2 | `password_file`、RADIUS 的 `secret_file`、`tls_key_file`、`tls_cert_file` 和 `ca_file` 必須是一般檔案，而且有大小上限（64 KiB；`tls_cert_file` 與 `tls_key_file` 是 1 MiB；`ca_file` 是 8 MiB）。 | FIFO、裝置、目錄和超過上限的檔案會被拒絕。指到一般檔案的連結還是可以用。 | 檢查每個這類路徑都是一般檔案、也沒超過上限。systemd 的 `LoadCredential`、Docker secrets 和 Kubernetes secrets 最後都是一般檔案（沒有一一試過）。 |
| 3 | 遠在未來的 heartbeat 算過期。 | 主機時鐘被往回調超過 `EMFOR_MAX_STALENESS`（預設 3 秒），而 daemon 之後又停了，查詢會把每個 backend 都看成未知：`closed` 的服務得到 `ALL_DOWN`（`emfor lookup` 結束代碼 1），`open` 的服務會回傳每個 backend。`emfor status` 的結束代碼從 0 變成 1。 | 時鐘正確的話，什麼都不用改。0.7 的 client 沒有這條規則。 |
| 4 | `heartbeat_age_ms` 可能是負的，在 `emfor status --json` 與 `emfor lookup --explain --json`（`store.heartbeat_age_ms`）裡。Rust 的 `emfor-client` 裡，`Explanation.heartbeat_age_ms` 是 `i64`（原本是 `u64`）。 | 預期這個值至少是 0 的解析程式或 Rust 程式，遇到比讀取端時鐘還晚的 heartbeat 就會出錯。 | 要能接受負數。 |
| 5 | `[peers] cipher = "aes-256-gcm"` 會送 `EMP4`。 | 舊版的成員會把 `EMP4` 當成 `malformed`。 | 設 `cipher` 之前，先升級群組裡的每個成員。 |

0.8.0 還有其他你可能會注意到的變更：

- **降級。** 設定檔如果用了 0.8 新增的 key（`[log] format`、`[peers] cipher`、`[metrics] allow`、`http` 檢查的 `ca_file`），0.7 的 `emfor check-config` 會失敗。降級前先拿掉。
- **log 變多了。** 一次 reload 最多寫 51 行它改了什麼（50 行，加一行說剩下的），每次啟動和 reload 也會記下它讀的每個設定檔的 SHA-256。
- **新的行與指標。** `emfor status` 和 `/metrics` 多了新的行與指標：聯防成員的時鐘偏差、不送結果的檢查者，以及 `emfor_clock_jumps_total`。daemon 也會記下主機時鐘的跳動（[第 12 章](12-operating.md)）。
- **`[metrics] allow`。** 不設就和以前一樣，只是 `listen` 如果是非 loopback 的位址，啟動與 reload 時會多一行警告。
- **空的 store。** daemon 從來沒寫過的 store，`emfor status` 現在說 `store is empty`，不再說成版本問題。

重新啟動前，每台主機都可以先快速檢查一下：

```sh
# emfor check-config                         # 用 0.8 的 binary
# stat -L -c '%F %s %n' /path/to/each-credential-file   # 是「regular file」嗎？大小在上限之內嗎？
# ldd /path/to/my-app | grep emfor           # 還連結著 libemfor.so.0.7 嗎？
```

## 從 0.6 到 0.7

store 格式（版本 2）和 plugin 協定（1）都沒有變：0.6 的 client 和 plugin 搭 0.7 的 daemon 還是能用，但還是請裝 0.7 的 client。

重新啟動 daemon **之前**，先把這份清單逐項處理完：

| # | 0.7.0 的變更 | 什麼會壞 | 該怎麼做 |
| --- | --- | --- | --- |
| 1 | `exec` 檢查與 plugin 只拿得到 `PATH`、`LANG`、`TZ`、`LC_*`（exec 另有 `EMFOR_*`）。 | 會讀 `HOME`、`http_proxy`、`LD_LIBRARY_PATH` 或其他變數的檢查程式，執行時就拿不到這些變數。`check-config` 不會警告。 | 把名稱加進 `check_options`（exec）或 `[plugins."<name>"]` 的 `env_pass`。沒有設 `ca_file` 的 `emfor-check` TLS 檢查，可能需要 `SSL_CERT_FILE` 或 `SSL_CERT_DIR`。 |
| 2 | store 檔案的預設權限改成 0640 和 0660（原本是 0644 和 0666）；daemon 每次啟動都會套用這些權限。 | 執行身分不在 daemon 群組裡的應用程式，會得到 `UNAVAILABLE: ... Permission denied`。 | 執行 `usermod -aG emfor <user>` 再重新啟動應用程式，或在 `[store]` 設 `file_mode = "0644"` 和 `lock_file_mode = "0666"`。 |
| 3 | 主設定檔也和 `conf.d` 裡的檔案一樣要經過檢查。 | 主設定檔如果 group 或其他人可寫，或擁有者是別的使用者，daemon 會起不來。 | `chmod g-w,o-w /etc/emfor/emfor.toml`、`chown root /etc/emfor/emfor.toml`。套件裝的檔案是 0644、擁有者 root。 |
| 4 | `encrypt` 是 `true` 時，`accept_plaintext` 預設為 `false`。 | 0.6 成員如果用 `encrypt = false` 執行，它送的明文訊息會被 0.7 成員丟掉。 | 整個群組都開啟加密；或在全部升級完之前，先在 0.7 成員上設 `accept_plaintext = true`。 |
| 5 | peer 密鑰檔的擁有者必須是 root 或 daemon 的使用者。 | `peers.secret_file` 會出現設定錯誤。 | `chown root /etc/emfor/peers.key`。 |
| 6 | 只有當 daemon 的紀錄寫明某把密鑰已被換掉時，`previous_secret_file` 才會用在那把密鑰上。 | 0.7 第一次啟動時還沒有紀錄，所以會忽略它。 | 不要在換密鑰換到一半時第一次啟動 0.7。 |
| 7 | 共用函式庫是 `libemfor.so.0.7`（soname 帶 minor 版本）。 | 連結 `libemfor.so.0.6` 的程式載入不了 0.7。 | 重新建置或重新安裝所有連結它的東西，包括 PHP 和 Perl 擴充模組。 |
| 8 | 時間長度改用 emfor 自己的解析器解析。 | 少數只有舊解析器接受的時間長度寫法，會讓 `check-config` 失敗。 | 改用 `"500ms"`、`"2s"`、`"1h 30m"` 這類寫法。 |

0.7.0 還有其他你可能會注意到的變更：

- **systemd unit。** 套件附的 unit 現在多了 `StartLimitIntervalSec=60`、`StartLimitBurst=5`、`UMask=0027`、`ProtectProc=invisible` 和 `ProcSubset=pid`。你自己手動裝的 `examples/systemd/emfor.service` 複本不會跟著變：請拿來和新版比對。一分鐘內啟動失敗五次後，unit 會停在 `failed`，要執行 `systemctl reset-failed emfor` 才會解除。
- **rpm。** `/etc/emfor/emfor.toml` 現在是 `%config(noreplace)`：升級時會保留你改過的檔案，套件附的版本則寫成 `emfor.toml.rpmnew`。（0.6 標的是 `%config`，會蓋掉改過的檔案，並把你的版本存成 `.rpmsave`；兩個版本附的檔案內容相同。）
- **http 檢查**在 `scheme = "http"` 時，現在在沒有 CA 憑證的主機上也能用。http 檢查失敗時，reason 裡不再帶 URL。
- `check-config` 和啟動時會多出這些**警告**（不是錯誤）：所有使用者都能讀的密鑰檔和密碼檔、不經 TLS 送出的密碼、讓其他使用者能寫入的 store 權限。
- **redis** 會在 reason 裡遮掉 AUTH 密碼；plugin 的 log 文字會跳脫；reason 裡看不見的格式字元也會跳脫。
- **writer lock 檔** `<store>.writer` 的權限是 0600。

重新啟動前，每台主機都可以先快速檢查一下：

```sh
# emfor check-config                         # 用 0.7 的 binary
# id www-data                                # 應用程式的使用者在 emfor 群組裡嗎？
# stat -c '%U %a %n' /etc/emfor/emfor.toml /etc/emfor/peers.key
# grep -rn 'check = "exec"' /etc/emfor/      # 有沒有需要 env_pass 的檢查程式？
```

## 更舊的版本

如果中間跳過了幾個版本，請把中間每一版的 CHANGELOG 都讀過。需要你動手的有：

| 版本 | 要做的事 |
| --- | --- |
| 0.6.0 | 0.5 的設定都能直接用。開始用 `drain = true` 之前，先升級 client，免得舊的 client 選到被 drain 的 backend。 |
| 0.5.0 | 設定驗證變嚴格了（`interval` 超過 `24h`、`timeout` 低於 `10ms` 等都會被拒絕）。先跑一次 `check-config`。 |
| 0.4.0 | store 格式是版本 2；0.4 以前的 client 每次查詢都會以 `INCOMPATIBLE` 失敗。peer 訊息預設加密；請先用 `encrypt = false` 一台一台升級整個群組，之後再開啟加密。 |

0.1 到 0.3 是早期版本；請見 [CHANGELOG.md](../../CHANGELOG.md)。
