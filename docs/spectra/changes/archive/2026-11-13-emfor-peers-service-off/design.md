## Context

聯防群組三個階段都已完成：`down` 通知觸發快速重檢、負責主機投票（區域判斷）、非負責主機靠投票分工。關閉的方法只有拿掉整個 `[peers]`，需要重啟，且所有服務一起退出。

目前的接線：

- 後端 task（crates/emfor-daemon/src/schedule.rs 的 `spawn_backend` 迴圈）每次檢查時看 `PeerLink.peers: Option<PeerHooks>`：`Some` 就算票（`quorum.votes` → `split`）、合併結果（`combine`）、送 `result` / `down`；`None` 就只用自己的結果。
- 設定值放在 `Settings`（`Copy`），經 `watch` 傳給後端 task，每圈 `*settings.borrow()` 讀一次；reload 時 `send_replace`，不重設狀態。
- `Supervisor::publish_targets` 把每個服務的 digest 與重檢通道放進 `Targets`，並以 `quorum.retain(&keep)` 丟掉已移除服務／後端的票、意見不一與 digest 不符。收到的 `down` 交給 `Targets::deliver`、`result` 先查 `Targets::backend`；服務不在 `Targets` 時訊息被忽略且不記 digest 不符。
- `meta/peers` 由 `Quorum::summary` 產生：啟動時 `peers::start_on` 算出 `initial_state` 交給 writer 與 `meta/started_at` 同一筆寫入；之後 peer task 每秒比對、有變才寫。
- `emfor status` 只讀 store（`config::load_store`），不讀服務設定，所以要靠 `meta/peers` 才知道哪些服務關了聯防。

## Goals / Non-Goals

**Goals:**

- 以服務為單位關掉聯防（`peers = false`），可寫在 `[defaults]` 或服務表，reload 立即生效、後端狀態保留。
- 關掉的服務在本機完全不收、不送、不用票；其他服務照常聯防。
- `meta/peers` 與 `emfor status` 看得出哪些服務關了。

**Non-Goals:**

- 不把關閉清單告訴其他成員（不改訊息格式、不改負責主機的選法）；所以其他成員仍會把本機列為該服務的候選負責主機。文件建議各成員設成一樣。
- 不提供「只送不收」或「只收不送」等半開狀態。
- 不新增指標；`emfor_peer_*` 照舊（關閉服務的訊息計入已收到、但被忽略）。
- 不改 `meta/schema_version`、client、DNS。

## Decisions

### `peers` 是一般服務設定，放進 `Settings`

`peers` 加進 `Settings` 與 `SETTING_KEYS`，和 `interval` 等一樣可寫在 `[defaults]` 與服務表，解析時要求 TOML 布林，否則錯誤 `peers must be true or false`。放進 `Settings` 的好處是 reload 走既有的 `send_replace`，後端 task 下一圈就讀到新值，不重設狀態；digest 不含它，所以改它不會讓成員互相判為 digest 不符。

考慮過另做一張 `[peers] exclude = [...]` 服務清單：集中好查，但和服務設定分離、`[defaults]` 無法表達「預設不參加、個別打開」，捨棄。

### 後端 task 以 `s.peers` 當成沒有 `PeerHooks`

迴圈開頭取 `let hooks = peers.peers.as_ref().filter(|_| s.peers);`，之後所有原本看 `peers.peers` 的地方（算票／split、`quorum.skip()`、`combine`、送 `result` / `down`）都改看 `hooks`。`peers = false` 時行為與沒有 `[peers]` 完全相同：每次自己檢查、結果原樣套用。重檢通道仍在 `select!` 裡，但因服務不在 `Targets`，不會有重檢送進來；已在進行的快速重檢照原本次數跑完（`fast_left` 不歸零）。

從 `false` 改回 `true` 時，`last_own_up` 與 `decided_in_row` 在關閉期間仍照常更新（每次都是自己檢查），所以打開後 split 的判斷直接可用，不需重設。

### Supervisor 從 `Targets` 與 `retain` 排除關閉的服務

`publish_targets` 只把 `settings.peers == true` 的服務放進 `Targets` 與 `keep`：關閉的服務收到 `down` / `result` 時查不到服務而忽略，也不記 digest 不符；既有的票、意見不一、digest 不符隨 `retain` 丟掉。關閉清單同時交給 `quorum.set_off(names)`，供 `meta/peers` 使用。`publish_targets` 在每次 `sync`（啟動與 reload）結尾都會呼叫，reload 改 `peers` 即生效。

### `meta/peers` 加 `services_off`，啟動值由 `peers::start` 帶入

`Quorum` 新增 `off: Vec<String>`（排序、去重），`set_off` 設定，`Summary` 在 `disagreements` 後加 `services_off`。啟動時 `meta/peers` 的值在 Supervisor 建立前就算好（`peers::start_on` 的 `initial_state`），所以 `peers::start` / `start_on` 多收一個參數 `services_off: Vec<String>`，由 cli 依設定算出並在算 `initial_state` 前 `set_off`。之後每秒的比對會在 reload 改清單後寫入新值。

### status 以 `services_off` 判斷，欄位缺少時當空

`PeerState` 加 `#[serde(default)] services_off: Vec<String>`；`is_voted` 與 `assigned_checkers` 遇到清單內的服務時當成沒有負責主機（不標 `voted`、`checkers` 為 `[]`、不計入 `checks n/m`）。舊版 daemon 寫的 `meta/peers` 沒有這個欄位，照舊判斷。

## Implementation Contract

**行為**

- 設定：`peers` 為布林，預設 `true`；`[defaults]` 與服務表皆可設，服務表優先。非布林 → `services."<name>".peers: peers must be true or false`（`[defaults]` 為 `defaults.peers: ...`）。沒有 `[peers]` 時接受且無作用。
- `peers = false` 的服務（有 `[peers]` 時）：不送 `down`、`result`；每次自己檢查，結果原樣套用（理由不加 `local:` / `peers d/f:`）；收到的 `down` 不觸發快速重檢、`result` 不保存；不記 digest 不符；`emfor_peer_checks_skipped_total` 不因它增加。
- reload 由 `true` 改 `false`：2 秒內該服務的意見不一與 digest 不符從 `meta/peers` 消失，`services_off` 列入該服務；後端狀態不變。改回 `true`：下一次檢查起重新參加。
- `meta/peers`：`{"listen":…,"members":[…],"digest_mismatch":[…],"disagreements":[…],"services_off":[…]}`，`services_off` 依名稱位元組序。
- `emfor status`：清單內服務的後端不標 `voted`、不計入 `checks n/m backends here`；`--json` 的 `assigned_checkers` 中其 `checkers` 為 `[]`。

**介面**

- `config::Settings { …, pub peers: bool }`，`Default` 為 `true`。
- `Quorum::set_off(&self, names: Vec<String>)`；`Summary.services_off`。
- `peers::start(config, site, targets, writer, services_off)` 與 `start_on(…, services_off)`。
- `status::PeerState.services_off: Vec<String>`（serde default）。

**驗收**

- config 單元測試：預設 `true`、`[defaults]` 繼承與覆寫、非布林錯誤訊息與路徑、沒有 `[peers]` 時接受。
- schedule 單元測試：`peers = false` 時（hooks 存在、票為 2 down）結果為自己的 `up`、不送 `PeerOut`、`quorum.skipped()` 不變；`true` 時照舊。
- supervisor 單元測試：關閉的服務不在 `Targets`、`quorum` 的意見不一被丟掉、`set_off` 收到清單；reload 改回後重新出現。
- quorum 單元測試：`summary` 含 `services_off`（排序）。
- status 單元測試：spec 的兩個新 scenario（文字無 `voted` / `checks`、JSON `checkers: []`），以及缺 `services_off` 時照舊。
- 整合測試（crates/emfor-daemon/tests/peers.rs）：三個 daemon，其中一個對服務設 `peers = false`；其他兩個看到 down、它保持 healthy，它不送 `result`（以對方的 `results_received` 或 `meta/peers` 無意見不一確認），`meta/peers` 的 `services_off` 正確。
- 每個新行為先讓測試在功能暫時關掉時失敗，再恢復通過。
- macOS 與 Debian 映像 `emfor-clients-test` 的 fmt、clippy、test 通過。

**範圍**

- 範圍內：config.rs、schedule.rs、supervisor.rs、quorum.rs、peers.rs（start 參數）、cli.rs（傳清單）、status.rs、tests/peers.rs、文件（03-configuration、01-concepts、08-operations、examples/emfor.toml、CHANGELOG）。
- 範圍外：訊息格式、負責主機選法、指標、client、DNS、store 版本。

## Risks / Trade-offs

- [其他成員仍把本機算成候選負責主機] → 某後端的三位負責主機若含本機，實際只有兩位會送票；兩票一致仍可決定（區域判斷要 2 票 down，分工要 2 票一致），票不足時非負責主機會自己檢查，只是少了容錯。文件寫明各成員設成一樣。
- [關閉服務收到的訊息仍計入 `emfor_peer_notifications_total`、`emfor_peer_results_total` 的已收到數] → 與「服務不存在」相同處理，屬既有語意；不另加指標。
- [reload 時正在進行的快速重檢] → 跑完剩下的次數再停；最多 `fall` 次、每 200ms 一次，影響有限。
