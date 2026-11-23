## Context

- `peers/task.rs` 的 `SecretSource` 管 `secret_file` 的讀取：目錄監看（inotify 或 kqueue）加每 30 秒的保險讀取，沒有監看時每次 heartbeat 前讀；讀取在 `SecretReader::refresh` 裡完成，結果交給 `apply_secret(group, read, warned, now)`。`refresh` 手上有兩個路徑：這次要讀的 `path`，以及等待中那次讀取當初讀的 `read_path`；`read_path` 與目前的 `path` 不同時（reload 在讀取進行中改了路徑）結果會被丟掉，不進 `apply_secret`。
- `apply_secret` 在 `group.keys.rotate(k, now)` 回 `true`（內容改變且有效）時，只記一行 `info`：`peer secret changed; messages are signed with the previous one for {SIGN_PREVIOUS_FOR}s more and it is accepted for {ACCEPT_PREVIOUS_FOR}s`。內容相同時 `rotate` 回 `false`，不記；讀不到或內容無效時記 `warn`（每分鐘最多一次）並保留舊密鑰。
- reload 的稽核（`audit.rs`）只比對設定：`secret_file`、`previous_secret_file` 的路徑字串有差異時記 `change=peers_changed`（`field`、`from`、`to`、`applies`）。它不讀密鑰內容，手冊第 12 章「What a reload changed」並寫明 log 不含密鑰內容或其雜湊。
- 啟動時另有幾行 `info`（`peer secret changed {N}s ago; ...`、`peers.previous_secret_file is ignored ...`），由 `start_keys` 記，不在這次範圍。
- `peer-group` 規格的「Peer secret reload」要求沒有規定輪換時要有 log 行（只規定備援讀取、讀取失敗、讀取逾時的 log）。

## Goals / Non-Goals

**Goals:**

- 內容輪換成功時，有一行與其他稽核行一致、可以機器過濾的 log：`change=peers_secret_rotated`，並指出是哪個檔案。
- 不洩漏任何與密鑰有關的內容：只有路徑。

**Non-Goals:**

- 不記雜湊、指紋或密鑰長度。
- 不改輪換行為、時間窗（簽 60 秒、收 120 秒）、讀取時機與逾時；不改 `previous_secret_file` 只在啟動讀。
- 不把這行放進 `audit.rs` 的 reload 稽核（它不是 reload 的結果，也不計入每次 reload 最多 50 行）。
- 不改啟動時的幾行 `info`。

## Decisions

### 在 `apply_secret` 記，由呼叫端傳入讀的路徑

`apply_secret` 加一個 `path: &Path` 參數，輪換成功時用它記 `secret_file`。`refresh` 的兩個呼叫點：一個處理上一次還沒完成的讀取，傳 `read_path`（此時已確認等於目前的 `path`）；一個處理剛讀完的結果，傳這次的 `path`。傳「讀的路徑」而不是「目前的路徑」，是因為這行要說的是這份內容來自哪個檔案；兩者在會被套用的結果上永遠相同（路徑不同的結果已被丟掉）。替代方案：在 `SecretSource` 上另外存路徑，再讓 `apply_secret` 讀它。放棄：結果要和讀它的路徑配對才不會記錯，傳參數比多一份可變狀態直接。

### 欄位與格式

用 `tracing::info!` 的結構化欄位：`change = %"peers_secret_rotated"`、`secret_file = %path.display()`，訊息文字不變。寫法與 `audit.rs` 的 `change = %"..."` 一致，所以 `change` 與路徑在 `[log] format = "json"` 下是獨立欄位，文字格式下顯示成 `change=peers_secret_rotated secret_file=/etc/emfor/peers.key`。新的 `change` 值是新增，不改既有值與欄位，符合手冊第 15 章對稽核欄位穩定性的承諾。

### 不經 reload，也不經 `audit.rs`

規格放在 `peer-group`（執行期行為），不放 `service-config` 的「Configuration audit log」（它只描述 reload 後的設定差異）。因此這行沒有 reload 的「在 `config reloaded` 之前」順序保證，也不受 50 行上限影響。

### 不含密鑰的證明方式

測試用已知的密鑰位元組，確認整份 log 不含它的任何形式（原文、十六進位、`sha256`）。路徑本身不是密鑰；reload 的 `peers_changed` 行已經寫出路徑。

## Implementation Contract

**行為：**

- 同一路徑的 `secret_file` 內容換成另一份有效密鑰後，在輪換套用的同時，記一行 `info`，欄位 `change=peers_secret_rotated`、`secret_file=<路徑>`，訊息文字與現在相同。
- reload 讓 `secret_file` 路徑改變，且新檔內容與目前密鑰不同時，記同一行，`secret_file` 是新路徑。內容相同、檔案被重寫但內容未變、讀不到或內容無效時，不記這行（無效仍照舊記 `warn`）。
- 這行不含密鑰內容、雜湊或指紋。

**資料形狀：** log 欄位 `change`（字串，`peers_secret_rotated`）、`secret_file`（字串，絕對路徑，與設定相同的寫法）。`apply_secret(group, read, path, warned, now)`。

**失敗與靜默：** 讀取失敗與無效內容的 `warn` 不變；同內容不記。這行每次輪換一次，不會在監看事件重複時重複記，因為 `rotate` 對相同內容回 `false`。

**驗收：** `peers/task.rs` 的單元測試用 `tracing_test::traced_test`：輪換成功有 `change=peers_secret_rotated` 與路徑且沒有密鑰內容；相同內容不記；無效內容不記；reload 改路徑後新路徑的輪換記新路徑。`audit/tests.rs` 加一個測試：`secret_file` 路徑改變產生 `field=secret_file`、`applies=now` 的 `peers_changed`。`tools/docker-test.sh` 全部通過。

**範圍：** 在範圍內：`peers/task.rs`、`audit/tests.rs`、`peer-group` 規格 delta、手冊 11 與 12 章中英、CHANGELOG。範圍外：上述 Non-Goals。

## Risks / Trade-offs

- **log 解析端：** 有人若依賴舊那行的純文字，訊息沒改，所以不受影響；新欄位只是多出來的。
- **路徑洩漏：** 路徑會出現在 log。reload 稽核行已經寫路徑，沒有新增的暴露。
- **`tracing_test` 對欄位的呈現：** 現有的 `tests/audit_log.rs` 直接比對不帶引號的 `change=backend_changed`，所以用 `%` 記的 `change` 在文字 log 裡就是這個樣子；`traced_test` 的 `logs_contain` 是否呈現相同，實作時以實際輸出為準，測試比對實際看到的形式。
