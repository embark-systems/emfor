## 1. 補測試（程式行為不變）

- [x] 1.1 在 `crates/emfor-check/src/options.rs` 的測試區新增測試：`read_password` 對只含 `\r\n` 的檔回 `password_file: file is empty`（既有 `read_password_errors_hide_the_content` 已涵蓋，不重複），對含 `caf\xe9\n` 的檔回 `password_file: not valid UTF-8`；對應 database-checks 的 requirement「Passwords from password_file only」的兩個「檢查時」情境，與「Check options validation」的 UTF-8 規則。驗證：在 Docker 跑 `tools/docker-test.sh` 的 emfor-check 測試通過，且新測試名稱出現在輸出中。
- [x] 1.2 在 `crates/emfor-plugin/src/lib.rs` 的 `check_results` 測試裡，把目標 `ip` 為 `nope` 的那筆（id 6）的斷言從只比對 `result == "error"` 加強為同時比對 reason 等於 `invalid target ip "nope"`；對應 checker-plugins 的 requirement「Plugin check results」的「Target ip is not an address」情境。驗證：在 Docker 跑 emfor-plugin 的測試通過。
- [x] 1.3 讓 `crates/emfor-daemon/tests/support/test_plugin.rs` 的測試 plugin 多出兩種行為，各以環境變數開關（比照既有 `EMFOR_TEST_PLUGIN_GARBAGE`）：(a) 收到 hello 時不回 hello，改寫一行 300 字元、不是 JSON 的 stdout（handshake 失敗用）；(b) 在 hello 回應之後另寫一行 300 字元、不是 JSON 的 stdout（一般無效行用）。不改任何既有模式的輸出。驗證：既有 `crates/emfor-daemon/tests/plugins.rs` 全部測試在 Docker 照舊通過。
- [x] 1.4 [after: 1.3] 在 `crates/emfor-daemon/tests/plugins.rs` 新增兩個測試：(a) 300 字元的無效 stdout 行，log 裡含該行前 200 個字元、不含 201 個連續相同字元，且有 `ignoring invalid line` 的 WARN；(b) 第一行不是有效 `hello` 的 300 字元 handshake 回應，`PluginHost::start` 失敗、錯誤訊息含前 200 個字元、不含 201 個（這個測試放在 `crates/emfor-daemon/tests/plugin_host.rs`，因為那裡直接呼叫 `PluginHost::start`）。對應 checker-plugins 的 requirement「Plugin output line limit」的「Long invalid stdout line」與「Long handshake answer」。驗證：在 Docker 跑 `crates/emfor-daemon` 的 plugins 測試，兩個新測試通過。
- [x] 1.5 確認既有 `long_reasons_are_cut`（`crates/emfor-plugin/src/lib.rs`）已斷言 70000 字元的 reason 被截成 `REASON_MAX_CHARS + 1`（513）個字元、以 `…` 結尾且整行小於 65536 位元組；已確認如此，不需改程式，這一項只記下對應 requirement「Plugin check results」的「Reason from a plugin crate is cut」情境。驗證：在 Docker 跑 emfor-plugin 的測試，`long_reasons_are_cut` 通過。

## 2. 規格差異核對

- [x] 2.1 [after: 1.1, 1.2, 1.4, 1.5] 逐條核對本變更兩份 delta spec 的新增與修改情境，各有一個通過的測試：stderr 1000 字元（既有 `oversized_stderr_line_is_cut`）、無效 stdout 與 handshake 200 字元（1.4）、crate 的 513 字元（1.5）、`invalid target ip`（1.2）、檢查時 `file is empty` 與 `not valid UTF-8`（1.1）、驗證時 `not valid UTF-8`（既有 `password_file_must_be_utf8`）。`check was cancelled` 沒有情境：只有 task 被中止或 runtime 關閉時才會發生，沒有實際可行的測試。驗證：列出每個情境與測試名稱的對照，沒有缺漏。
- [x] 2.2 [after: 2.1] 執行 `tools/docker-test.sh` 跑完整測試，並執行 `spectra validate emfor-spec-code-alignment`。驗證：兩者都通過；不在主機上執行 cargo。
