## 1. emfor-format：候選層級說明

- [x] 1.1 為 spec 要求「Explain shows the tier and the reason of every backend」，在 `crates/emfor-format/src/rules.rs` 的 `healthy_or_all` 旁新增層級說明函式：回傳候選來自哪一層（通過檢查的 primary、通過檢查的 backup、含失敗中的 primary、含失敗中的 backup、`on_all_down` 為 `open` 時的通過檢查的 `unknown`、全部 `unknown`、全部 backend、或沒有），以及每個 backend 為什麼在或不在候選裡（`unhealthy`、`unknown`、`last check failed`、`backup held back`）。驗證：`cargo test -p emfor-format` 新增的單元測試，對 `healthy_or_all_table` 的每一列，說明函式給出的候選與 `healthy_or_all` 相同，並檢查每個 backend 的原因（spec lookup-command 的「tier of the candidates」範例表）。
- [x] 1.2 把 `healthy_or_all` 改寫成呼叫 1.1 的函式並只取候選，簽名與回傳值不變，使層級規則只有一份。驗證：現有的 `healthy_or_all_table` 測試不改一個字仍通過；`cargo test -p emfor-client --all-features`（含 `tests/conformance.rs`）與 `cargo test -p emfor-daemon --lib select::` 通過；`tests/conformance/v1/cases.json` 的案例不需要修改。 [after: 1.1]

## 2. emfor-client：共用前置步驟與 explain

- [x] 2.1 抽出共用的前置步驟，讓 lookup 與 explain 走同一條路：把 `Client::lookup_once`（`clients/rust/src/client.rs`）中從讀 store 到算出候選的步驟抽成 lookup 與 explain 共用的內部步驟，並讓 `Client`（建構函式 `Client::new(path, max_staleness_ms)`）從 crate 公開，`lookup` 的行為與四個錯誤碼完全不變，全域的 `emfor_client::lookup` 與 `libemfor` 的 C API 不變。驗證：`cargo test -p emfor-client --all-features` 的既有測試一個字不改仍通過（含 `env_*`、`fork*`、`conformance*`、`same_lmdb`）。 [after: 1.2]
- [x] 2.2 explain 不推進任何狀態（spec 要求「Explain does not change any state」；同時滿足「Explain shows the tier and the reason of every backend」）：新增 `Client::explain(name, affinity_key)`：回傳 store 路徑、heartbeat 年齡與是否 stale、`balance`、`on_all_down`、候選的層級、候選與被排除的 backend（含原因）、挑選規則、是否確定與確定時的那一台（`failover` 與有 key 的 `sticky`）；對輪流類的 `balance` 列出候選與規則（`weighted` 的權重比例、`locality` 的站台子集、`fastest` 的快組、慢啟動的步數與有效權重），不呼叫 `Client::next`。驗證：新增的單元測試涵蓋 spec 的「Primary and backup backends」「Stale store」「Closed service」情境與層級範例表、`failover` 與有 key 的 `sticky` 的確定結果，並有一個測試證明在 explain 前後，同一個起點的兩個 client 的 `lookup` 輪流序列相同。 [after: 2.1]

## 3. emfor-daemon：`emfor lookup` 子命令

- [x] 3.1 為 spec 要求「Lookup command answers like the client」與「Lookup command exit status」，在 `crates/emfor-daemon/Cargo.toml` 新增對 `emfor-client` 的依賴；`cli.rs` 新增 `lookup` 子命令（`<service>`、`--key`、`--explain`、`--json`、`--dns A|AAAA`），新模組以 `--config` 的 `[store] path`（與 `emfor status` 相同）建立 `emfor_client::Client`，印出一行 `ip:port` 並以 spec 的結束碼表結束，不寫 store。驗證：`crates/emfor-daemon/tests/` 新增整合測試（用現有的 store 寫入輔助工具造 store）：`failover` 與有 key 的 `sticky` 的答案與 `Client::lookup` 相同、`round-robin` 的答案落在候選裡、唯讀的 store 也能查、結束碼表每一列（0、1、2、3）。 [after: 2.2]
- [x] 3.2 為 spec 要求「JSON output is stable and only grows」，做 `--explain` 的文字輸出與 `--json` 輸出（欄位 `service`、`via`、`family`、`store`、`balance`、`on_all_down`、`result`，加 `explain` 的 `tier`、`candidates`、`excluded`、`selection`），失敗時仍印出已知的說明。驗證：整合測試逐欄位檢查 JSON 的名稱與型別（spec 的「JSON for a fixed choice」「JSON for a rotating choice」），並檢查 `closed` 服務沒有 healthy backend 時結束碼 1、標準輸出沒有位址、但有層級與原因。 [after: 3.1]
- [x] 3.3 為 spec 要求「DNS mode explains the answer of the DNS server」，做 `--dns A|AAAA`：用 `crates/emfor-daemon/src/select.rs` 的 family 與候選函式，先依 family 過濾再算層級、store 一律視為 fresh、`sticky` 沒有 key 時當 `round-robin`、`weighted` 以 smooth weighted round-robin 說明、沒有該 family 的 backend 時結束碼 1、`SRV` 或未知值為用法錯誤（結束碼 2），輸出明講「讀的是 store，與 DNS 伺服器的記憶體最多差約一秒」且不說下一個是誰。驗證：整合測試涵蓋 spec 的「A and AAAA have different tiers」「No backend of the family」「Unknown family」三個情境。 [after: 3.2, 1.2]
- [x] 3.4 確認 `emfor server` 的程序不經過 `emfor_client` 的全域 client：`crates/emfor-daemon/src` 裡呼叫 `emfor_client` 的只有 `lookup` 模組，而且不使用 `emfor_client::lookup`（它會走 `global()` 並在 FreeBSD 註冊 `pthread_atfork` 處理函式）。驗證：`grep -rn "emfor_client" crates/emfor-daemon/src` 的結果只在 lookup 模組；FreeBSD 的實機確認留到發版前（見 5.1）。 [after: 3.3]

## 4. 文件

- [x] 4.1 中英手冊（`docs/manual/`、`docs/manual.zh_TW/`）：第 6 章說明 `emfor lookup` 與 `--explain`、輪流類 `balance` 的答案是「一個新程序會看到的」、cron 工作要穩定結果就用 `failover` 或 `sticky` 加 `--key`；第 8 章加入結束碼與 `--json`；導讀列出新指令；DNS 模式寫明讀 store、最多差約一秒、不做 SRV。驗證：兩種語言的標題與連結錨點存在且對應（內容審閱與連結檢查）；範例輸出與 spec 的範例一致。 [after: 3.4]
- [x] 4.2 `docs/deploy/daemon.md` 新增 `emfor lookup` 一節；`docs/spectra/LANGUAGE.md` 新增「候選層」並說明與 `candidates` 的關係；`CHANGELOG.md` 的 Unreleased 記錄新子命令、`emfor-client` 公開 `Client` 與 `explain`、`emfor-daemon` 新增依賴但不新增安裝檔案；更正 `clients/rust/src/select.rs` 的「Pure functions」與 `crates/emfor-daemon/src/select.rs` 的「No I/O」註解（兩者都會推進輪流計數器）。驗證：CHANGELOG 條目與 spec 一致，且不宣稱沒量測的事（例如 `emfor` 二進位檔的大小）；註解改完後 `cargo clippy` 無新警告。 [after: 4.1]

## 5. 驗證

- [x] 5.1 在 Debian docker 映像 `emfor-clients-test`（必要時用 `docker build -q -t emfor-clients-test tests/clients` 重建）執行 `cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 與 `cargo test --workspace --all-features`，並執行 `spectra validate emfor-lookup-explain`。驗證：全部通過；macOS 與 FreeBSD（含 `emfor server` 與 fork 測試在 FreeBSD 上不受新依賴影響）等發版前再測，回報時明說這次沒有測。 [after: 4.2]
