## Context

emfor 的設定只能是一個 TOML 檔案。`crates/emfor-daemon/src/config.rs` 的 `load(path, registry)`、`load_store(path)`、`parse(text, registry)` 都是「讀檔、解析成 `toml::Table`、交給 `Validator` 建出 `Config`」；沒有 include、目錄、glob，也沒有環境變數展開。`/spectra-discuss`（2026-11-15，advisor 參與）核對過下面這些事實，都是讀原始碼的結果，沒有執行測試：

- `config.rs` 的 `ConfigError` 只有 `path` 與 `message` 兩欄（`path` 的格式由 `key_path` 產生，例如 `services."api.svc.emfor".backends[0]`）；錯誤由 `Validator.errors` 收集；`unknown_keys` 只看傳進來的那個表，無法知道鍵來自哪個檔案。
- `crates/emfor-daemon/src/cli.rs` 的 `load` 讀一次主設定檔文字，之後把同一段文字交給 `config::parse_plugins`、`parse`、`parse_store`、`parse_dns_listen`、`parse_metrics_listen`、`parse_peers_listen`，這六個各自重新解析（它們都以 `text.parse::<toml::Table>()` 開頭）。`load_store` 只驗證 `[store]`，`emfor status` 與 `emfor lookup` 都呼叫它。
- 聯防群組的服務定義 digest 由 `check_digest(checker_id, check_options, timeout)` 計算，輸入是合併後的值（`ServiceConfig` 裡的 `check_options` 與 timeout），所以只依內容、不依檔案切分。
- 對設定檔本身沒有權限檢查；`secret_file` 有（`read_secret`：其他人可讀就拒絕）。
- `toml` 是 1.2.1，預設功能；`toml::de::Error::span()` 可用，`Table` 不帶位置。
- 設定測試有 32 個，風格是 `parse_ok(text)`／`parse_err(text)`。

手冊「What a reload changes」那張表說明 reload 能套用什麼；這個 change 不改那張表，只是 reload 的對象變成合併後的設定。

## Goals / Non-Goals

**Goals:**

- 一個 `include_dir` 目錄裡的多個檔案依檔名順序合併成一份設定，後面的蓋掉前面的。
- 合併後的設定走與單一檔案完全相同的驗證、plugin 啟動、reload 的「需要重啟」檢查與 digest。
- 錯誤能指出檔案；`check-config` 與日誌列出讀了哪些檔案。
- 沒有設定 `include_dir` 時，行為與輸出和現在完全相同。

**Non-Goals:**

- 不印出合併後的有效設定與各鍵來源、不支援多個目錄、不允許後面的檔案刪除前面的東西、不對主設定檔加權限檢查、不做 `drain = true`、不改聯防群組的設定、不改 reload 能套用什麼。

## Decisions

### 一個函式讀全部檔案，合併成一份 table，所有解析都吃它

新增一個讀取函式：讀主設定檔，若頂層有 `include_dir`，再依規則讀目錄裡的檔案並合併，回傳一份 `toml::Table`，加上「鍵路徑對檔案」的對照表與讀過的檔案清單（以及警告，例如略過的非 UTF-8 檔名）。`load`、`load_store`（只讀主檔）、`parse`、`check-config` 與 reload 全部用它；`parse_plugins`、`parse_store`、`parse_dns_listen`、`parse_metrics_listen`、`parse_peers_listen` 這五個輔助函式與 `parse` 一律新增一個吃 `&toml::Table` 的版本。

原因：如果只有 `parse` 看到合併後的設定，那麼片段裡宣告的 plugin 不會被啟動（`parse_plugins` 讀的是主檔文字），片段裡改 `[dns]`、`[metrics]`、`[peers]` 的 listen 也躲得過 reload 的「需要重啟」檢查（`parse_*_listen` 讀的是主檔文字）。

進行順序是先新增、再遷移、最後收掉：先加新的讀取函式與吃 table 的輔助函式，文字版保留並讓它們呼叫 table 版（`parse(text, &registry)` 這個文字入口留著，現有 32 個設定測試不必修改）；再把 `cli.rs` 的 `load` 逐一遷移；最後移除不再被呼叫的文字專用路徑。

### 合併規則

只有「表加表」才逐鍵遞迴合併（`[services."x"]`、它的 `check_options`、`[defaults]`、`[dns]`、`[metrics]`、`[peers]`、`[log]`、`[plugins."x"]`）。其他一律由後面的值整個取代：純量、清單（包含 `backends` 與 `command`）、以及型別不同的值（表對純量、表對清單）。行內表與 `[x.y]` 區段解析結果相同，所以合併方式相同（測試涵蓋）。頂層純量（例如 `site`）也是後面的取代前面的。

這表示 `99-local.toml` 可以只改某個服務的 `interval`，但要改 `backends` 就得寫完整的清單。合併後缺 `check` 或 `backends` 的服務，與現在一樣是驗證錯誤。

### 哪些檔案、什麼順序

讀 `include_dir` 目錄裡的一般檔案，以及指向一般檔案的符號連結（設定管理工具常用），名稱以 `.toml` 結尾、不以 `.` 開頭。順序是檔名的位元組字典序（`Path` 比較，不依地區），所以 `9-a.toml` 排在 `10-a.toml` 之後；手冊寫明要補零（`01-`、`10-`、`99-`）。檔名不是 UTF-8 的略過並記警告。其他所有東西（`foo.toml~`、`.x.toml.swp`、`x.toml.disabled`、子目錄）都忽略，所以把檔案改名就能讓它不被讀。相對的 `include_dir` 相對於主設定檔所在目錄；一個目錄，不接受清單；值必須是字串。目錄是空的合法（0 個片段）。

### 只准在主設定檔的鍵

`[store]` 與 `include_dir` 只准出現在主設定檔；片段含有這兩者就是錯誤，訊息指出片段的檔名。原因：`emfor status` 與 `emfor lookup` 只讀主設定檔的 `[store]`（`load_store`），片段若能改它，這兩個指令會讀錯 store；而 `include_dir` 只在主檔也順便排除遞迴 include。後果要寫進手冊：只用 `status` 與 `lookup` 的人只需要能讀主設定檔，不需要能讀 `conf.d`。其他表與頂層 `site` 可以出現在片段裡並合併。

### 權限：直接拒絕

`include_dir` 的目錄，或其中任何被讀的檔案（符號連結取指向的檔案），其 `mode & 0o022` 不為 0（group 或其他人可寫）就拒絕載入，錯誤指出路徑與模式。原因：片段可以定義 `[plugins."x"].command` 或 `exec` 檢查，能寫那個目錄的人就能以 daemon 的身分執行程式。主設定檔本身不加新檢查：它現在沒有，現有安裝可能是 `0664`，加了會讓升級後起不來。這是檢查範圍的不對稱（主檔不檢查、`conf.d` 檢查），在手冊說明，由管理者決定是否另外收緊主檔。

### 目錄不存在是錯誤

`include_dir` 指向的路徑不存在或不是目錄，就是設定錯誤。套件安裝時建立這個目錄（FreeBSD port 的 `PLIST` 加 `@dir` 並在 `Makefile` 建立，deb、rpm、tarball 同理；從原始碼安裝時手冊要說明 `mkdir`），所以套件附的預設設定可以直接啟用 `include_dir`。啟動時：記錄錯誤並以結束碼 2 結束，與其他設定錯誤相同。reload 時目錄消失：拒絕這次 reload、保留舊設定繼續執行、記錄錯誤（既有的「reload rejected; keeping the previous config」）；daemon 不結束，因為目錄被誤刪就讓所有主機同時失去健康檢查，比設定沒更新糟得多。

### 錯誤歸屬，不用位置資訊

合併時，對每個葉節點（純量、清單）記下「鍵路徑到最後設定它的檔案」，鍵路徑用同一個 `key_path` 格式。`Validator::err` 收到錯誤的 `path` 後，向上找最近有紀錄的祖先鍵，把那個檔案附在錯誤上；`ConfigError` 新增一個選用的檔案欄位，`Display` 在有檔案時顯示。TOML 語法錯誤由各檔案單獨解析，用 `toml::de::Error::span()` 換算成 `檔名:行:欄`。誠實的限制：當一個錯誤由兩個檔案的值合起來造成（例如 `timeout` 在一個檔案、`interval` 在另一個檔案），歸屬只是盡力而為，指向最後設定該鍵的那個檔案。

替代方案：改用帶位置資訊的型別（`toml::Spanned`）或另一個 TOML 套件（`toml_edit`）。拒絕：`Table` 不帶位置，要整個驗證流程改型別，工作量大，換來的只是欄位層級的行號。

### check-config 與日誌

`emfor check-config` 的第一行不變（`config ok: <n> services, <m> backends`）；只有在設定了 `include_dir` 時，後面多印讀過的檔案，每個一行（`read <path>`），主設定檔在前，片段依序；沒設定 `include_dir` 時輸出與現在完全相同（寫腳本解析輸出的人不受影響）。daemon 在啟動與每次成功的 reload 用 `info` 日誌記下同一份清單。

### 審查後補充的決定（ingest）

review 與 advisor 在實作後指出四個行為，規格原本沒寫，現在補進 spec delta（實作與測試已完成）：

- 空字串的 `include_dir` 直接拒絕（`include_dir: must not be empty`），否則 `parent.join("")` 會把主檔所在目錄當成片段目錄。
- 目錄裡檔案的 metadata 讀取失敗時，只有「不存在」（懸空連結）略過，其他錯誤都是載入錯誤；以自我指向的連結測試（以 root 執行的 docker 無法重現 EACCES）。
- 錯誤歸屬延伸到 plugin 啟動失敗與 reload 的「需要重啟」錯誤：在 `load` 裡統一用 `Layers::origins` 補上檔案。
- 略過檔名的警告：`check-config` 沒有 log，改印到 stderr；daemon 不論載入成功與否都在啟動與 reload 時記錄，因為被略過的檔案可能就是失敗的原因。已知缺口：`read_layers` 本身失敗時，先前收集的警告不會輸出。

### 不受影響的東西

聯防群組的服務定義 digest 來自合併後的 `check_options` 與 timeout，不依檔案切分；以測試守住（同一個服務拆成兩個檔案與單一檔案的 digest 相同）。reload 的流程不變：重讀主檔與全部片段，任何一個地方有錯就整份拒絕。設定管理工具應原子寫入片段（先寫暫存檔再改名）：reload 遇到寫到一半的檔案只會拒絕、保留舊設定，不會壞掉，手冊要建議這樣做。

## Implementation Contract

**Behavior**

- 設定了 `include_dir` 時，daemon 讀主設定檔與目錄裡符合條件的檔案，合併後驗證；沒設定時行為與現在完全相同（包含 `check-config` 的輸出）。
- 合併後的設定與把同樣內容寫在單一檔案裡的設定，在驗證、plugin 啟動、reload 的「需要重啟」檢查與 digest 上結果相同。
- 片段含 `[store]` 或 `include_dir`、目錄或被讀的檔案 group 或其他人可寫、目錄不存在，都是錯誤，載入失敗。
- reload 時任何一個檔案有錯，整份拒絕並保留舊設定；目錄消失也是如此，daemon 不結束。

**Interface / data shape**

- 新的頂層選用鍵 `include_dir`（字串）。
- `config.rs`：新的讀取並合併函式（回傳 table、鍵路徑對檔案的對照、檔案清單、警告）、吃 `&toml::Table` 的 `parse`、`parse_plugins`、`parse_store`、`parse_dns_listen`、`parse_metrics_listen`、`parse_peers_listen`；`ConfigError` 新增選用的檔案欄位。
- `check-config`：在設定了 `include_dir` 時，於 `config ok: ...` 行之後，每個讀過的檔案一行 `read <path>`。

**Failure modes**

- 語法錯誤：`檔名:行:欄` 與 TOML 的錯誤訊息。驗證錯誤：鍵路徑、訊息與（盡力而為的）檔名。
- 啟動時任何設定錯誤：結束碼 2，不開 store。reload 時：記錄 `error`、保留舊設定。
- 無法讀取目錄裡的某個檔案（權限、讀取錯誤）：該檔案的錯誤，整份載入失敗。

**Acceptance criteria**

- 合併規則表、順序、檔名篩選、主檔專屬的鍵、權限、目錄不存在、plugin 在片段中宣告、片段改 listen 被 reload 檢查擋下、錯誤帶檔名與 `file:line:col`、`check-config` 列出檔案、digest 分檔與單檔相同、reload 重讀片段並在任何錯誤時拒絕、`status` 與 `lookup` 在片段不可讀時仍可用，每一項都有測試（見 tasks）。
- 現有 32 個設定測試不修改仍通過。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 在 Debian docker（`tools/docker-test.sh`）通過；`tools/check-manual-links.py` 通過；macOS 與 FreeBSD 等發版前再測。

**Scope boundaries**

- 範圍內：`config.rs`、`cli.rs`（`load`、check-config、reload 的錯誤處理與日誌）、`service-config` 規格、手冊第 2、3、8、10 章（中英）、`docs/deploy/daemon.md`、`examples/emfor.toml`、`examples/freebsd`、CHANGELOG。
- 範圍外：有效設定的傾印、多目錄、刪除語意、主設定檔權限檢查、`drain = true`、聯防群組設定、reload 能套用什麼。

## Risks / Trade-offs

- **改動面**：`load` 的六個文字重新解析要全部遷移，漏掉任何一個就會出現「片段裡的東西沒生效」的靜默錯誤。以 plugin 在片段宣告、片段改 listen 這兩個測試守住，並在最後移除文字專用路徑，讓編譯器找出漏網之魚。
- **權限檢查的相容性**：新增的 `0o022` 檢查只針對 `include_dir`，因為是新功能，沒有升級後起不來的問題；主設定檔不檢查是刻意的不對稱。
- **歸屬不精確**：兩個檔案的值合起來才出錯時只是盡力而為。可以接受，錯誤訊息仍帶鍵路徑，足以定位。
- **符號連結**：權限取指向的檔案，但目錄裡的連結本身可以由有目錄寫入權的人改指，所以目錄的權限檢查是必要的前提。
- **舊版 daemon**：讀到 `include_dir` 會以 unknown key 失敗；混合版本的叢集會立刻發現，可以接受。
