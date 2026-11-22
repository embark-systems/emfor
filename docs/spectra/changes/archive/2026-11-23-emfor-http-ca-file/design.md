## Context

`http` 檢查在 `crates/emfor-daemon/src/checker/http.rs`：`ALLOWED` 列出可用選項（`path`、`scheme`、`host_header`、`expect_status`、`tls_verify`、`expect_body`），`parse_options` 解析，`HttpChecker::client_for` 為每個後端建一個 `reqwest::Client`（rustls，信任系統 CA 庫；建立時在 `spawn_blocking` 裡讀系統憑證）。`skip_cert_verification(https, tls_verify)` 決定要不要關掉驗證（`tls_verify = false` 或 `scheme = "http"`）。其他帶 TLS 的檢查已有 `ca_file`，由 `crates/emfor-tls` 提供 `check_ca_file`（設定驗證時用）與 `trusted_certificates`（取得要信任的憑證）；`tcp` 檢查的做法是 `ca_file` 缺 `tls = true` 時報 `requires tls = true`，`tls_verify = false` 與 `ca_file` 同時設則不衝突。

## Goals / Non-Goals

**Goals:**

- 私有 CA 簽發的 https 後端能用 `ca_file` 通過 `http` 檢查，不必動系統信任庫、不必關驗證。
- 與其他檢查的 `ca_file` 同名、同語意、同驗證訊息。

**Non-Goals:**

- 其他檢查的 `ca_file`、client certificate、`/metrics` 保護、SNI 與 `server_name`。
- 「加在系統 CA 之上」的模式：沒有第二個選項，`ca_file` 就是取代。

## Decisions

### ca_file 取代系統 CA，不是附加

設了 `ca_file` 就只信該檔裡的憑證（reqwest 的 `tls_certs_only`，不讀系統庫）。和其他檢查、`curl --cacert` 一致，封閉機房裡也最嚴格。同一個檢查要同時信公開與私有端點時，把需要的 CA 都放進同一個 PEM 檔。替代方案：附加到系統 CA（`tls_certs_merge`）混用較方便，但與其他檢查的行為不同，已放棄。

### 憑證從 emfor-tls 取，不另寫 PEM 解析

驗證（`parse_options`）呼叫 `emfor_tls::check_ca_file`，訊息與其他檢查相同；建 client 時用 `emfor_tls::trusted_certificates` 取得憑證，轉成 reqwest 的 `Certificate` 後交給 `tls_certs_only`。這樣 PEM 的寬容度（略過非憑證區塊）、錯誤訊息與快取行為只有一份。

### scheme = "http" 設 ca_file 是驗證錯誤

沒有 TLS 就沒有憑證可驗，設了 `ca_file` 必是設定錯誤，早點報：選項 `ca_file`，訊息 `requires scheme = "https"`。與 `tcp` 的 `requires tls = true` 同型。

### ca_file 與 tls_verify = false 同時設不算錯

`tls_verify = false` 優先，不驗證；`ca_file` 仍通過格式驗證（檔案要存在且有效）。與 `tcp` 檢查一致，也方便在不改 CA 檔的情況下暫時關掉驗證。

### 換掉 ca_file 不必 reload

其他檢查的 `ca_file` 在檔案變更（inode、大小、mtime、ctime）後下次檢查就重讀，不必 reload（`emfor-tls` 的 `client_config`）。http 的 `reqwest::Client` 是快取的（每個設定或每個後端一個），若不處理，換了 CA 檔要 reload 才生效，與其他檢查不一致，輪替私有 CA 時容易漏。決定：每次檢查先對 `ca_file` 做一次 `stat`，用同一個「檔案是否變更」的定義（`emfor-tls` 把它以一個小的公開函式提供，不在 `http.rs` 另寫一份）；變了就丟掉快取的 client，下次重建。替代方案：只在 reload 時重建（簡單，但與其他檢查的行為不同）；已放棄。

### 檢查時 CA 檔失效是 error，不是 down

建 client 時讀不到或解析不出憑證（驗證之後檔案被刪或換壞），結果是 `error`，原因說明 `ca_file` 與路徑，不是 `down`（`down` 表示後端失敗）。這與其他檢查對本機 TLS 失敗的處理一致，也不會把後端記成不健康。

## Implementation Contract

**行為**

- `ca_file = "/etc/emfor/ca.pem"`、`scheme = "https"`：後端憑證由該檔裡的 CA 簽發則 `up`（依 `expect_status`），未由其簽發（含由系統 CA 簽發）則 `error` 或 `down` 依現行的 TLS 失敗分類（與沒設 `ca_file` 的憑證失敗相同）。
- 沒設 `ca_file`：行為與現在完全相同。
- `scheme = "http"` 設 `ca_file`：驗證失敗，錯誤在 `ca_file`，訊息 `requires scheme = "https"`；同時 `scheme = "http"` 與 `ca_file` 之外的選項錯誤照常一起回報。
- `ca_file` 不是字串、不是絕對路徑、檔案不存在、不可讀或沒有任何憑證：驗證失敗，訊息同其他檢查的 `ca_file`（`not an absolute path`、`file not found`、`cannot read: <kind>`、`no certificate found`、`no usable certificate`）。
- 驗證之後把 `ca_file` 換成另一個有效的 PEM 檔，下一次檢查就用新的，不必 reload；沒變時不重建 client。
- 設了 `ca_file` 時，建 client 失敗的提示不再寫「需要系統 CA 憑證」，改指出 `ca_file`。

**介面**

- `ALLOWED` 加入 `"ca_file"`；`HttpOptions` 加 `ca_file: Option<PathBuf>`；`client_for` 在 `ca_file` 為 `Some` 且驗證開啟時用 `tls_certs_only`。
- `emfor-tls` 新增一個公開函式回報 `ca_file` 的變更標記（dev、inode、大小、mtime、ctime），`http.rs` 用它判斷快取的 client 還能不能用。
- 設定檔文件：`ca_file | string | (none) | With https: PEM file of CA certificates to trust, instead of the system's`。

**失敗模式**

- 驗證階段：以 `OptionError` 回報，`check-config` 失敗，daemon 不載入。
- 檢查階段（檔案之後壞了）：`error`，不計入後端失敗。

**驗收標準**

- `crates/emfor-daemon/src/checker/http.rs` 的選項單元測試：`ca_file` 有效檔案通過；`scheme = "http"` 加 `ca_file` 得 `requires scheme = "https"`；不存在的檔案、非字串、無憑證各得與其他檢查相同的訊息；`ca_file` 加 `tls_verify = false` 通過。
- 整合測試（本機起一個用測試 CA 簽發憑證的 HTTPS 伺服器，憑證由 `emfor-tls` 的 `testing` feature 產生）：沒設 `ca_file` 時憑證不被信任，設了含該 CA 的 `ca_file` 時 `up`，設了不含該 CA 的檔時失敗。
- 全部測試經 `tools/docker-test.sh` 在 Debian Docker 內通過。

**範圍**

- 範圍內：`http.rs` 的選項、client 建立、錯誤提示、health-checking spec 的 HTTP checker requirement、手冊第 6 與 16 章（中英）、CHANGELOG。
- 範圍外：Non-Goals 列的各項。

## Risks / Trade-offs

- **取代系統 CA 的代價**：想同時信公開與私有端點的人要自己把兩邊的 CA 放進同一個 PEM 檔。手冊要說明。
- **每次檢查多一次 `stat`**：為了讓換掉的 `ca_file` 不必 reload 就生效，每次檢查要對檔案做一次 `stat`（見上一節的決定）；檔案沒變就沿用快取的 client，不重讀、不重建。
- **測試用憑證**：`crates/emfor-tls` 有 `testing` feature（`rcgen`）可產生測試 CA 與伺服器憑證；整合測試用它，伺服器端的 TLS 接受端由測試自己組（現有的 `crates/emfor-daemon/tests/http_without_ca.rs` 只涵蓋沒有系統 CA 的情況，沒有 HTTPS 伺服器）。這部分的工作量在 apply 階段評估。
