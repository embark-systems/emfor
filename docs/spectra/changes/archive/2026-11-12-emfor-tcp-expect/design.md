## Context

`tcp` 檢查（`TcpChecker`）目前只做 `TcpStream::connect`：連上即 up 並立刻關閉，`TcpFactory::validate` 以 `reject_unknown_keys(options, &[], …)` 拒絕所有選項。逾時由核心的 `run_check` 統一處理（未完成即 down `timeout`），檢查器本身不必計時。`fastest` 會把每次 up 檢查的耗時記進 `latency_us`。

需要確認服務「有在回應」而不只是「port 有開」時，目前只能用 `exec`（每次檢查起一個程序）或外掛。討論的結論是在內建 `tcp` 加上單步驟的 `send` / `expect`，比照 HAProxy `tcp-check expect string`。

## Goals / Non-Goals

**Goals:**

- `tcp` 檢查可選擇在連上後送出一段字串，並要求回應中出現指定子字串。
- 只設 `expect` 時可檢查伺服器主動送出的歡迎訊息（SMTP、FTP、SSH）。
- 不符合時的原因帶有收到內容的開頭，方便判斷後端回了什麼。
- 不設選項的 `tcp` 服務行為完全不變。

**Non-Goals:**

- regex 比對（`expect_regex`）：daemon 目前沒有連結 regex，之後需要再加。
- TLS（`tls`、`tls_verify`、`server_name`、`ca_file`）：HTTPS 由 `http` 檢查處理，資料庫由 emfor-check 處理。
- 多步驟對話（HAProxy 的 connect / send / expect 序列）：之後可用 `steps = [...]` 擴充並保留單步驟寫法。
- 二進位內容（十六進位寫法的 `send` / `expect`）。
- 符合後送出結束指令（例如 SMTP `QUIT`）：符合即直接關閉連線。

## Decisions

### 單步驟 send 與 expect 選項

只提供一組 `send`（可選）與 `expect`。替代方案是 HAProxy 式的步驟陣列；它要處理每一步的逾時、失敗發生在哪一步、原因格式，複雜度高很多，而目前想得到的用途（banner、一問一答）單步驟都能涵蓋。

### expect 用子字串位元組比對

`expect` 的 UTF-8 位元組必須出現在收到的位元組中，區分大小寫，可跨多次 read。替代方案是 regex（需新相依、使用者還得處理跳脫）或「開頭必須是」（SMTP 多行歡迎訊息、memcached 回應順序都讓它不實用）。

### 跳脫交給 TOML 字串

`send` 與 `expect` 是一般 TOML 字串，原樣使用其 UTF-8 位元組；`"stats\r\n"` 讀進來就是 CR LF。不另設跳脫規則，避免兩層跳脫互相干擾（例如反斜線要寫幾個）。

### 讀取上限 16384 位元組

最多保留 16384 位元組；讀滿仍未出現就 down，不等到逾時。這同時限制記憶體用量並讓「一直送資料但不對」的後端很快被判定。因為上限，`expect` 本身也不能超過 16384 位元組（驗證時拒絕），否則永遠不可能符合。16384 是估計值：一般歡迎訊息與 `stats` 這類回應遠小於此（memcached `stats` 約 1–2 KB，未量測）；實作後以單元測試確認上限行為，不據此做其他決定。

### 不符合時的原因帶前 64 位元組

原因格式 `expect: got "<excerpt>"`，取收到內容的前 64 位元組，跳脫規則與 Rust `<[u8]>::escape_ascii` 相同（可列印 ASCII 原樣，`\t \r \n`，`\\ \' \"`，其他 `\xNN` 小寫）。取開頭而非結尾，是因為協定的錯誤碼通常在開頭（`ERROR`、`-NOAUTH`、`500`）。64 位元組讓原因保持一行可讀。

### 送出或讀取出錯的原因前綴

寫入失敗為 down `send: <錯誤>`，讀取失敗（不含對方正常關閉）為 down `expect: <錯誤>`；本機資源錯誤（`is_local_io_error`）仍為 error，與連線階段一致。

## Implementation Contract

**行為**

- `check = "tcp"` 且沒有 `check_options`：與現在相同，連上即 up，立刻關閉。
- 有 `expect`：連上 → 若 `send` 非空則寫入其位元組（一次寫完）→ 持續讀取並累積，最多 16384 位元組 → 累積內容中出現 `expect` 即 up 並關閉連線。
- 讀到 EOF 或累積滿 16384 位元組仍未出現：down，原因 `expect: got "<前 64 位元組，escape_ascii>"`。
- 寫入出錯：down `send: <io 錯誤文字>`；讀取出錯：down `expect: <io 錯誤文字>`；兩者若為本機資源錯誤（`is_local_io_error`）則為 error。
- 超過服務 `timeout`：由 `run_check` 回 down `timeout`（不變）。

**介面 / 資料形狀**

- `TcpFactory::validate` 接受鍵 `send`、`expect`，其餘鍵仍為 `unknown key`。
- 驗證錯誤（鍵、訊息）：

| 條件 | 鍵 | 訊息 |
| --- | --- | --- |
| `send` 不是字串 | `send` | `must be a string` |
| `expect` 不是字串 | `expect` | `must be a string` |
| 有 `send` 沒有 `expect` | `expect` | `required when send is set` |
| `expect` 為空字串 | `expect` | `must not be empty` |
| `expect` 超過 16384 位元組 | `expect` | `must be at most 16384 bytes` |

- 驗證後的設定（例如 `TcpChecker { send: Vec<u8>, expect: Option<Vec<u8>> }`）在 `validate` 時建好，`check` 不再解析選項。

**驗收**

- `crates/emfor-daemon/src/checker/tcp.rs` 的單元測試以本機假伺服器涵蓋 spec 中每個情境與 Example：SMTP 歡迎訊息 up、memcached `stats` up 且伺服器收到的正好是 7 個位元組、分兩次寫入的 `+PONG` up、關閉時的四種原因、送滿 16384 位元組不等逾時即 down、沉默的伺服器經 `run_check` 得到 `timeout`、五種驗證錯誤；原有 `listening_backend_is_up`、`closed_port_is_down_refused`、`rejects_any_option` 保持通過。
- 本機檢查：macOS 與 Debian 映像 `emfor-clients-test` 的 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 通過。

**範圍**

- 範圍內：`tcp` 檢查器、其單元測試、`examples/emfor.toml` 範例、`docs/spectra/config.yaml` 的檢查器說明。
- 範圍外：`http`、`exec`、外掛、LMDB 格式、client、DNS；Non-Goals 列出的功能。

## Risks / Trade-offs

- [符合後直接關閉連線，部分伺服器會記一行「連線中斷」日誌] → 文件註明；需要乾淨結束的協定改用 `exec` 或外掛。
- [`fastest` 服務的 `latency_us` 會包含等待回應的時間，數值比單純連線大] → 這反映服務真正的回應速度，屬預期；文件不需另外處理。
- [子字串可能意外出現在錯誤訊息中，例如 `expect = "OK"` 符合 `NOT OK`] → 由使用者選擇夠明確的字串；範例使用 `+PONG`、`STAT pid` 這類不易誤判的文字。
- [`send` 沒有長度上限] → 值來自管理者寫的設定檔，不另設限制。
