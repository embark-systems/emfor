## Why

有些服務的後端和主機之間的網路距離不同（另一個機櫃、另一個機房、比較慢的副本），目前沒辦法讓應用程式優先使用從這台主機回應最快的後端。每台主機上的 emfor daemon 本來就從應用程式所在的主機做檢查，所以它可以記錄檢查延遲、判斷哪些後端和最快的差不多快，再讓 client 和 DNS 優先使用它們。emfor-balance-2 把這件事延後，等兩個問題有答案：量到的延遲有多不穩定，以及怎麼避免同一台主機上所有 client 都湧向唯一最快的那台後端。

## What Changes

- 新的分配方式 `fastest`，服務表格和 `[defaults]` 都可以設定。
- daemon 記錄每次 `up` 檢查的耗時，用指數移動平均平滑（新樣本權重 0.05），後端變成不健康時清空。
- daemon 為每個 `fastest` 服務判斷哪些後端「快」：延遲在最低者的 1.5 倍或加 1 ms 以內時進入，超過 2 倍且超過最低者加 2 ms 才離開。離開門檻比較寬（遲滯），避免門檻邊緣的後端每次檢查都進進出出。
- store 中每個後端物件多一個 `latency_us`（整數微秒或 `null`），給維運人員看。`fastest` 服務的記錄多一個 `pool`：daemon 選出的後端 id。`meta/schema_version` 維持 `1`；舊 client 忽略這兩個欄位，遇到 `fastest` 改用 round-robin。
- 以 `fastest` 挑選時，讀取端不做任何判斷：client（Rust client，以及透過 libemfor 的 Go、Python、Perl、PHP 包裝）在 `pool` 裡以 round-robin 輪流；`pool` 空的或 store 心跳過期時，等同 round-robin。DNS server 用同一份 `pool` 回答；SRV 中 `pool` 成員給 priority 0，其他給 priority 1。
- 實驗（task 2.1）用真實的 tcp 與 http 檢查量測延遲抖動，包含一台明顯較慢的後端，以確認或修改常數。沒有通過門檻的話，change 就停在那裡並記錄結果。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `backend-selection`：`fastest` 成為有效的 balance 值；daemon 判斷快組成員並發布為 `pool`；daemon 對 `fastest` 的 A/AAAA 選擇使用它。
- `state-store`：後端物件帶 `latency_us`，`fastest` 記錄帶 `pool`；延遲如何量測、平滑與清空。
- `client-lookup`：`fastest` 的 client 在 `pool` 裡輪流；共用一致性測試資料涵蓋六種分配方式。
- `dns-server`：`fastest` 的 SRV priority 對應。
- `service-config`：設定檢查接受 `fastest`，錯誤訊息列出六個值。

## Impact

- Affected specs: backend-selection, state-store, client-lookup, dns-server, service-config.
- Affected code: crates/emfor-format/src/lib.rs, crates/emfor-daemon/src/config.rs, crates/emfor-daemon/src/schedule.rs, crates/emfor-daemon/src/state.rs, crates/emfor-daemon/src/writer.rs, crates/emfor-daemon/src/select.rs, crates/emfor-daemon/src/dns.rs, clients/rust/src/select.rs, clients/rust/src/client.rs, tests/conformance/v1/cases.json, tools/emfor-fixture/tests/conformance_data.rs.
- Affected docs: docs/spectra/LANGUAGE.md, docs/spectra/config.yaml, docs/deploy/dns-resolver.md, examples/emfor.toml, clients/rust/README.md, clients/c/README.md.
- 不新增相依套件。舊 client 對新 daemon 照常運作（本來就容忍不認得的欄位與 balance 值）。
