## Why

內建 `tcp` 檢查只看 TCP 連線能否建立：服務程序卡住、但 kernel 還在幫它接受連線時，仍然算 up。要確認服務真的有在回應，目前只能用 `exec` 每次起一個程序（例如 `nc` 加 `grep`），或另寫外掛。HAProxy 的 `tcp-check expect` 是常見做法；emfor 先做單步驟版本，就能涵蓋大多數會先送歡迎訊息的服務（SMTP、FTP、SSH），以及一問一答的文字協定（memcached `stats`、redis `PING`）。

## What Changes

- 內建 `tcp` 檢查新增兩個可選的 `check_options`：
  - `send`：連上後送出的字串，原樣送出位元組；`\r\n` 等跳脫由 TOML 字串本身處理，不另做跳脫。
  - `expect`：回應中必須出現的子字串（位元組比對，區分大小寫）。
- 組合：
  - 只設 `expect`：連上後只讀不寫，檢查伺服器主動送出的歡迎訊息。
  - `send` 與 `expect`：先送出，再讀回應。
  - 只設 `send`、`expect` 為空字串、`expect` 超過 16384 位元組、或值不是字串：設定錯誤。
  - 兩者都不設：和現在完全相同（連上即 up）。
- 判定：持續讀取直到累積的回應中出現 `expect` 就是 up，並立刻關閉連線。對方先關閉連線、或讀滿 16384 位元組仍未出現，是 down，原因為 `expect: got "<收到內容的前 64 位元組，經跳脫>"`；送出或讀取時連線出錯是 down，原因以 `send: ` 或 `expect: ` 開頭；超過 `timeout` 仍照現有規則為 `timeout`。
- 文件：`examples/emfor.toml` 加上 banner 與 send/expect 範例；`docs/spectra/config.yaml` 的檢查器段落記錄 tcp 的新選項。

## Non-Goals

詳見 design.md 的 Goals / Non-Goals：不做 regex、TLS、多步驟對話、二進位內容比對。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `health-checking`：「TCP checker」需求由「不接受任何 `check_options`」改為接受 `send` 與 `expect`，並定義 expect 的判定、原因格式與驗證錯誤。

## Impact

- 程式：crates/emfor-daemon/src/checker/tcp.rs（選項驗證、送出與讀取比對、單元測試）。
- 不新增相依套件；不影響 LMDB 格式、client、DNS 與外掛協定。
- 文件：examples/emfor.toml、docs/spectra/config.yaml。
- 現有只設 `check = "tcp"` 的服務行為不變；原本設了任何 `check_options` 的 tcp 服務本來就是設定錯誤，因此沒有相容性問題。
