## Context

四個互不相干的小修，共同點是「輸入沒有被當成不可信的資料」或「測試假設了時序」，所以放在一個 change。設計只記下有取捨的決定；其餘照 proposal。

## Goals / Non-Goals

**Goals:**

- `removed_directory_is_a_failure` 在負載下不再偶發失敗，且仍然驗證它原本要驗證的事（目錄被刪之後，監看回報失敗）。
- 手動觸發 fuzz 的 `seconds` 不會被當成 shell 程式碼。
- pf 腳本只把合法的 IPv4／IPv6 地址交給 `pfctl`；Nagios 腳本的輸出不會被 `reason` 或名稱裡的字元弄亂。

**Non-Goals:** 見 proposal 的 Non-Goals。

## Decisions

### flaky 測試：輪詢等失敗，不改 SecretWatch

`notified` 只代表「有事件」，不代表「已失敗」；inotify 的 `wait` 對任何事件都返回，目錄被刪的失敗在稍後的讀取才設定。測試新增一個輔助函式 `failed(&w)`：最多 3 秒（與 `notified` 相同），每 10 ms 看一次 `w.failure()`，出現就返回 `true`。`removed_directory_is_a_failure` 改為 `assert!(failed(&w).await, "the watch reports its failure")`，不再先 `notified`（失敗的設定會 `notify_one`，但不需要依賴它）。

替代方案：在 `SecretWatch` 裡先記錄失敗再 `notify`，或讓 `wait` 只在失敗時返回。不採用：生產程式碼的行為是對的（失敗在設定時一併通知；事件通知本來就可以早於失敗），要修的是測試的假設。

### fuzz.yml：env 變數加數字檢查

`Fuzz` 步驟加 `env: FUZZ_SECONDS: ${{ github.event.inputs.seconds || 300 }}`，`run:` 裡先 `case "$FUZZ_SECONDS" in ''|*[!0-9]*|0*) echo "seconds must be a number above 0, without a leading 0" >&2; exit 1 ;; esac`（`0` 會被 libFuzzer 當成不限時，job 會跑到上限才被砍，所以也擋掉），再用 `-max_total_time="$FUZZ_SECONDS"`。`${{ matrix.target }}` 與 `${{ matrix.args }}` 不動（來自檔案本身，不是外部輸入；`matrix.args` 需要被分詞成多個旗標）。

### pf 腳本：在 awk 裡驗證，無效的略過並說一行

awk 已經逐行讀 SRV 記錄；驗證放在同一處，讓無效的目標在進入集合之前就被丟掉：

- IPv4：第一個標籤符合 `^[0-9]+-[0-9]+-[0-9]+-[0-9]+$`，換成點之後四個八位元組各 ≤ 255。
- IPv6：第一個標籤以 `-` 分成恰好 8 組，每組是 1–4 個十六進位字元（`^[0-9A-Fa-f]+$` 且長度 ≤ 4）。emfor 產生的目標名稱是 8 組各 4 位數字，這個檢查比它寬鬆一點，只擋掉明顯不是地址的字串；不處理 `::` 縮寫（emfor 不產生）。
- 其餘：略過並寫一行到 stderr（`ignoring SRV target <name>: not an address`，名稱以 `%s` 印出、經 `tr -cd '[:print:]'` 去掉控制字元）。
- `EMPTY` 與資料表名稱：啟動時以同一個驗證（地址）與 `^[A-Za-z0-9_][A-Za-z0-9_-]*$`（表名；不以 `-` 開頭，免得被 `pfctl` 當成旗標）檢查，不合就 `die`。
- `pfctl` 失敗：保留輸出的第一行放進 `die` 的訊息，不再丟掉。
- `$want`：驗證之後只剩地址，沒有 glob 字元；仍用不加引號的分詞，並在那個函式裡 `set -f`（關掉 pathname expansion），移除 `# shellcheck disable=SC2086` 以外的任何依賴由註解說明為什麼安全。

替代方案：用 `printf '%s\n' "$want" | xargs pfctl ...`。不採用：多一個行程與不同的錯誤處理，沒有安全上的好處。

### Nagios 腳本：在 jq 裡做兩個小函式

`def text: gsub("[|]"; "/") | gsub("[\n\r]"; " ");` 用於外掛輸出文字裡來自 emfor 的每個片段（服務名稱、後端 id、reason）；`def perflabel: gsub("[|]"; "/") | gsub("'"; "''") | gsub("[\n\r]"; " ");` 用於 perfdata 的標籤（`label` 是 jq 保留字，所以不能用這個名字；標籤也要處理 `|` 與換行，否則仍會提早結束）。`unknown()` 的訊息（來自 emfor 的 stderr 第一行）同樣把 `|` 換成 `/`。理由：Nagios 外掛規格以 `|` 分隔文字與 perfdata、以 `'` 包住含空白的標籤、以 `''` 表示標籤內的 `'`；其餘字元不需處理。不跳脫 `=`（服務名稱依 emfor 的設定驗證不會含 `=`；若含，perfdata 會多一個欄位，而不是被注入）。

## Implementation Contract

- **行為（測試）**：`removed_directory_is_a_failure` 在 3 秒內等到 `failure()` 為 `Some` 才通過；重複跑完整的 `tools/docker-test.sh` 不再出現這個失敗。
- **行為（fuzz）**：`seconds` 不是純數字、是 0 或以 0 開頭時該步驟以 1 結束並印一行；是數字時與修改前相同。
- **行為（pf）**：對一個含 `10-0-0-11`（合法）、`10-0-0-300`（八位元組超出）、`evil-name`、`2001-db8-0-0-0-0-0-1`（合法 IPv6）的假 SRV 回答，`once` 只把合法的兩個地址交給（假的）`pfctl`，並為其他兩個各印一行警告；`EMPTY=127.0.0.1 ` 之類不合法的值使腳本在啟動時 `die`；假 `pfctl` 失敗時，`die` 的訊息含它輸出的第一行。
- **行為（Nagios）**：reason 為 `a|b` 的後端在輸出裡 `|` 之前只有一個 `|`（文字與 perfdata 之間的那個），且文字裡是 `a/b`；服務名稱含 `'` 時 perfdata 標籤裡是 `''`。
- **介面**：兩個腳本的命令列、輸出格式、結束碼不變；`tests/pf/validate.sh` 是新的、可在任何有 `sh`、`awk` 的系統上跑的測試（不需要 FreeBSD 或 pf）。
- **失敗模式**：被略過的 SRV 目標不使整個更新失敗；全部都被略過時等同於「沒有地址」（`NONE`）。
- **驗收**：(a) `tools/docker-test.sh` 全過（含重跑 3 次 `peers::watch` 相關測試）；(b) 在 Docker（Debian）裡跑 `tests/pf/validate.sh` 通過；(c) `tests/nagios/run.sh` 通過（Docker）；(d) `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/fuzz.yml"))'` 與 `sh -n` 檢查 `run:` 區塊；(e) `python3 tools/check-manual-links.py`。
- **範圍**：見 proposal 的 Impact 與 Non-Goals。

## Risks / Trade-offs

- pf 驗證比 emfor 實際產生的名稱寬鬆一點（任何 1–4 位十六進位的八組），所以不是完整的 IPv6 驗證；`pfctl` 仍是最後一道檢查，而它的錯誤現在會被看見。
- 輪詢的間隔（10 ms）與期限（3 秒）是沿用 `notified` 的選擇，未量測。
- 未驗證：FreeBSD 上真實的 `drill` 輸出格式與 `pfctl`（假的測試替身只重現了腳本讀的那幾欄）、真實 Nagios 對跳脫後輸出的解析、GitHub Actions 上 `fuzz.yml` 的實際執行（只做 YAML 與 shell 的語法檢查）。
