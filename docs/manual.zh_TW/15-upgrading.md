# 15. 升級

[手冊](README.md) · 上一章：[14. 整合](14-integrations.md) · 下一章：[16. 疑難排解](16-troubleshooting.md)

## 規則

- 所有元件共用同一個版本號並一起發行：daemon、`emfor-check`、`libemfor`、Rust crate，以及 Go、Python、Perl 與 PHP client。**請從同一個發行版安裝它們，並一起升級。**`libemfor` 內含 daemon 所用的 LMDB 建置；版本不符會得到 `UNAVAILABLE`，絕不會得到錯誤的資料。
- 1.0 之前，minor 版（0.6 → 0.7）可能改變穩定介面；CHANGELOG 會把每個這樣的變更列在 **Read before you upgrade** 之下，或標為 *breaking* / *compatibility change*，並寫出該怎麼做。patch 版絕不需要你這邊做任何變更。
- store 每次啟動都會重建，所以從來沒有需要搬移的 store；只有狀態歷史會保留下來。
- 設定檔會拒絕不認得的 key。降版之前，先移除舊版不認得的 key。
- 穩定的：設定檔、指令與結束代碼、`status --json`、store 格式、client API、plugin 協定、DNS 回答、metric 名稱。不穩定的：`emfor status` 的文字、log 行、錯誤訊息的措辭。script 裡請比對代碼與結束代碼。見 [docs/compatibility.md](../compatibility.md)。

## 每台主機上的順序

1. **先升級 client。** 安裝新的 `libemfor` 與包裝函式庫，並重新啟動應用程式讓它們載入（執行中的行程會一直用它啟動時的函式庫）。
2. **用新的 binary 檢查設定**：`emfor check-config`。可以的話以 daemon 的使用者執行。設定無效時，systemd unit 的 `ExecStartPre` 會拒絕啟動，daemon 就停著。
3. **安裝 daemon 並重新啟動。** 用 restart，而不是停止、安裝、再啟動，讓空窗短一點（[第 12 章](12-operating.md#重新啟動時應用程式會看到什麼)）。
4. **確認**：`emfor status` 以 0 結束，log 裡沒有你預期之外的警告。

一次只處理一台主機。

## 從 0.6 到 0.7

store 格式（版本 2）與 plugin 協定（1）沒有改變：0.6 的 client 與 plugin 搭配 0.7 的 daemon 仍可運作，但還是請安裝 0.7 的 client。

重新啟動 daemon **之前**，逐項處理這份清單：

| # | 0.7.0 的變更 | 什麼會壞 | 該怎麼做 |
| --- | --- | --- | --- |
| 1 | `exec` 檢查與 plugin 只拿到 `PATH`、`LANG`、`TZ`、`LC_*`（exec 另有 `EMFOR_*`）。 | 讀取 `HOME`、`http_proxy`、`LD_LIBRARY_PATH` 或其他變數的檢查程式，會在沒有這些變數的情況下執行。`check-config` 不會警告。 | 把名稱加進 `check_options`（exec）或 `[plugins."<name>"]` 的 `env_pass`。沒有 `ca_file` 的 `emfor-check` TLS 檢查可能需要 `SSL_CERT_FILE` 或 `SSL_CERT_DIR`。 |
| 2 | store 檔案的預設權限為 0640 與 0660（原本是 0644 與 0666）；daemon 每次啟動都會設定權限。 | 使用者不在 daemon 群組裡的應用程式會得到 `UNAVAILABLE: ... Permission denied`。 | `usermod -aG emfor <user>` 並重新啟動應用程式，或在 `[store]` 設定 `file_mode = "0644"` 與 `lock_file_mode = "0666"`。 |
| 3 | 主設定檔和 `conf.d` 的檔案一樣會被檢查。 | group 或其他人可寫、或由別的使用者擁有的主設定檔，會讓 daemon 無法啟動。 | `chmod g-w,o-w /etc/emfor/emfor.toml`、`chown root /etc/emfor/emfor.toml`。套件安裝的檔案是 0644、root 擁有。 |
| 4 | `encrypt` 為 `true` 時，`accept_plaintext` 預設為 `false`。 | 0.7 的成員會丟棄以 `encrypt = false` 執行的 0.6 成員送來的明文訊息。 | 在整個群組開啟加密，或在全部升級完成之前，於 0.7 成員上設定 `accept_plaintext = true`。 |
| 5 | peer 密鑰檔必須由 root 或 daemon 的使用者擁有。 | 在 `peers.secret_file` 出現設定錯誤。 | `chown root /etc/emfor/peers.key`。 |
| 6 | `previous_secret_file` 只用於 daemon 的紀錄中寫明它已換掉的那把密鑰。 | 0.7 第一次啟動時沒有紀錄，所以會忽略它。 | 不要在更換密鑰的途中第一次啟動 0.7。 |
| 7 | 共用函式庫是 `libemfor.so.0.7`（soname 帶有 minor 版本）。 | 連結 `libemfor.so.0.6` 的程式無法載入 0.7。 | 重新建置或重新安裝連結它的東西，包括 PHP 與 Perl 擴充模組。 |
| 8 | 時間長度由 emfor 自己的解析器解析。 | 只有舊解析器接受的少見時間長度寫法，會讓 `check-config` 失敗。 | 使用 `"500ms"`、`"2s"`、`"1h 30m"` 這類寫法。 |

0.7.0 其他你可能會注意到的變更：

- **systemd unit。** 套件附的 unit 現在有 `StartLimitIntervalSec=60`、`StartLimitBurst=5`、`UMask=0027`、`ProtectProc=invisible` 與 `ProcSubset=pid`。你手動安裝的 `examples/systemd/emfor.service` 複本不會改變：請和新的比較。一分鐘內啟動失敗五次之後，unit 會停在 `failed`，直到 `systemctl reset-failed emfor`。
- **rpm。** `/etc/emfor/emfor.toml` 現在是 `%config(noreplace)`：升級會保留你改過的檔案，並把套件附的檔案寫成 `emfor.toml.rpmnew`。（0.6 標成 `%config`，會取代改過的檔案，並把你的存成 `.rpmsave`；兩版附的檔案相同。）
- **http 檢查**在 `scheme = "http"` 時，現在可以在沒有 CA 憑證的主機上運作。失敗的 http 檢查的 reason 不再包含 URL。
- `check-config` 與啟動時的**警告**（不是錯誤）：每個使用者都能讀的密鑰檔與密碼檔、不經 TLS 送出的密碼、讓其他使用者能寫入的 store 權限。
- **redis** 會在 reason 中隱藏 AUTH 密碼；plugin 的 log 文字會被跳脫；reason 中看不見的格式字元會被跳脫。
- **writer lock 檔** `<store>.writer` 的權限是 0600。

重新啟動前，在每台主機上快速檢查：

```sh
# emfor check-config                         # 用 0.7 的 binary
# id www-data                                # 應用程式的使用者在 emfor 群組裡嗎？
# stat -c '%U %a %n' /etc/emfor/emfor.toml /etc/emfor/peers.key
# grep -rn 'check = "exec"' /etc/emfor/      # 有需要 env_pass 的檢查程式嗎？
```

## 更舊的版本

如果跳過了某些版本，請讀中間每一個 CHANGELOG 項目。需要處理的有：

| 版本 | 要做的事 |
| --- | --- |
| 0.6.0 | 接受每一份 0.5 的設定。使用 `drain = true` 之前先升級 client，以免較舊的 client 選到被 drain 的 backend。 |
| 0.5.0 | 設定的驗證更嚴格（高於 `24h` 的 `interval`、低於 `10ms` 的 `timeout` 等都會被拒絕）。先執行 `check-config`。 |
| 0.4.0 | store 格式是版本 2；早於 0.4 的 client 每次查詢都以 `INCOMPATIBLE` 失敗。peer 訊息預設加密；請以 `encrypt = false` 一台一台升級群組，之後再開啟加密。 |

0.1 到 0.3 是早期版本；見 [CHANGELOG.md](../../CHANGELOG.md)。
