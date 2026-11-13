## Context

emfor-peers-service-off 讓單一服務以 `peers = false` 不參加聯防，但刻意不告訴其他成員（peer-group「Services left out of the peer group」原文：other members keep counting this daemon as a candidate）。結果是：主機 A 只在自己這邊關掉服務 S 時，其他成員仍可能把 A 選成 S 某些後端的負責主機，而 A 不會送 `result`，那些後端實際只剩兩台負責主機有票。

現況（已讀過程式碼）：

- crates/emfor-daemon/src/peers.rs 的 `Wire` 沒有 `deny_unknown_fields`，舊版會忽略新欄位；`MAX_DATAGRAM` 為 1200；heartbeat 在 peer task 的每秒 tick 以 `group.encode(Kind::Heartbeat, ..)` 產生；`decode` 把 `"heartbeat"` 解成沒有內容的 `Kind::Heartbeat`；`received()` 對 heartbeat 不做事。
- crates/emfor-daemon/src/quorum.rs 的 `assigned(service, backend, me, live)` 以 `me` 加 `live` 為候選；`Quorum::combine` 與 `Quorum::votes` 都以 `st.live` 呼叫它；`Quorum::set_off` 保存本機關閉的服務名稱，`Quorum::retain` 收到本機參加聯防的服務與後端。
- crates/emfor-daemon/src/supervisor.rs 的 `publish_targets` 在啟動與每次 reload 呼叫 `quorum.retain` 與 `quorum.set_off`。
- crates/emfor-daemon/src/status.rs 的 `assigned_checkers` 以 `meta/peers` 的 `listen` 與 `live` 成員呼叫 `quorum::assigned`。
- crates/emfor-daemon/Cargo.toml 已相依 `sha2`。

## Goals / Non-Goals

**Goals:**

- 其他成員知道某主機不參加哪些服務，選負責主機時把它排除，讓仍有 3 台候選的後端保有 3 張可能的票。
- `emfor status` 的 `voted` 與 `assigned_checkers` 與 daemon 的選法一致。
- 與舊版成員混用時不出錯：舊版照舊行為，新版不因舊版的訊息而丟棄任何東西。

**Non-Goals:**

- 不新增訊息種類（例如另送一則 `services` 訊息）：heartbeat 每秒都送、自然會在對方重啟後重送，不需另做重送與確認。
- 不送完整服務名稱：名稱最長可達數百 bytes，放不進一個 datagram；短雜湊固定 8 個字元。
- 不改 rendezvous 公式、負責主機數量（3）與「候選不到 3 台就沒有負責主機」的規則。
- 不處理雜湊碰撞以外的名稱比對（大小寫、別名）：服務名稱照原樣雜湊。
- 不讓關閉清單影響故障通知（`down`）的收送：本機關閉的服務原本就不送、不理會 `down`。

## Decisions

### 以 heartbeat 的 off 欄位傳送服務標籤

heartbeat 加選填欄位 `off`，內容是本機 `peers = false` 服務的「服務標籤」：UTF-8 名稱 SHA-256 的前 4 bytes，寫成 8 個小寫十六進位字元，去重後依字串排序。沒有關閉的服務時不帶欄位，所以一般群組的 heartbeat 位元組完全不變。

替代方案：(a) 送完整名稱——長度不固定，名稱長時放不下；(b) 另設訊息種類——要處理重送與對方重啟，heartbeat 已經每秒重送；(c) 用 MD5——與 rendezvous 同一個雜湊函式易混淆，SHA-256 與檢查定義摘要一致。

### 每則 heartbeat 最多 80 個服務標籤

最壞情況的 heartbeat（計算值，未實測）：`from` 為 IPv6 含 scope id 約 58 bytes、`site` 63 bytes、`seq` 20 位數、`ts` 13 位數，payload 約 233 bytes，加上 36 bytes 標頭與 `,"off":[]` 9 bytes 約 278 bytes；每個標籤 11 bytes（含引號與逗號），80 個為 879 bytes，總計約 1157 bytes，低於 1200。上限用固定數字而不是「放得下就放」，讓結果與 `seq` 位數無關、可預測，也讓 warn 只在清單改變時出現。實作時以單元測試量測：用最長的 `from`、`site`、`seq` 與 80 個標籤編碼 heartbeat，長度必須 ≤ 1200；若測試失敗，降低上限並更新 spec。

超過 80 個時送排序後的前 80 個，啟動與每次 reload 改變關閉清單時記一行 `warn`，說明幾個沒送出；沒送出的服務維持舊行為（其他成員仍把本機算作候選）。

### 成員的服務標籤由最新 heartbeat 取代並在不存活時清空

每收到一則 heartbeat 就以它的 `off` 取代該成員保存的標籤（沒有欄位即清空）；`down`、`result` 與未知種類不改變；成員不再存活時清空。如此對方 reload 後一秒內生效，重啟後的第一則 heartbeat 也會立即更正。`off` 格式錯誤的 heartbeat 以 `malformed` 丟棄（與其他欄位錯誤一致），不只忽略欄位，以免把格式錯誤當成「沒有關閉的服務」。

### 候選排除在呼叫 assigned 之前完成

`quorum::assigned(service, backend, me, live)` 的簽名與演算法不變；呼叫端先把「保存的標籤含有該服務標籤」的存活成員從 `live` 拿掉再傳入。`Quorum::combine`、`Quorum::votes` 與 status 的 `assigned_checkers` 共用同一個方式，所以 rendezvous 的既有測試與 spec 範例全部保留。兩個本機服務標籤相同時，宣告該標籤的成員在兩者都被排除（32 位元標籤、100 個服務的碰撞機率約 1/860,000，計算值）。

### meta/peers 的成員多 services_off 名稱

`meta/peers` 的 `members[]` 每一項多 `services_off`：本機服務（參加與不參加聯防的都算）中，服務標籤落在該成員保存標籤裡的名稱，依名稱排序。寫名稱而不是標籤，讓維運人員直接看得懂，也讓 `emfor status` 不必重算雜湊；本機沒有的服務的標籤不顯示。status 讀取時缺欄位視為空，舊 store 照舊。

## Implementation Contract

**行為：**

- 本機有 `peers = false` 的服務時，每秒的 heartbeat 帶 `"off":[<標籤>...]`；reload 改變關閉清單後，下一則 heartbeat（1 秒內）反映新清單。
- 存活成員宣告服務 S 的標籤時，本機選 S 各後端的負責主機不把它算作候選；它停止宣告或不再存活後，從下一次檢查起又算作候選。
- `meta/peers` 在內容改變後 2 秒內寫入新的 `members[].services_off`。
- `emfor status` 的 `voted`、`checks n/m backends here` 與 `--json` 的 `assigned_checkers` 使用相同排除。

**介面與資料形狀：**

- 線上格式：`Wire` 加 `off: Option<Vec<String>>`（`skip_serializing_if = "Option::is_none"`）；`Kind::Heartbeat` 帶本機要送的標籤（例如 `Kind::Heartbeat(Vec<u32>)` 或等價形式），`decode` 驗證每個元素是 8 個小寫十六進位字元。
- `quorum.rs`：服務標籤函式（名稱 → `u32`）；`Quorum::set_off` 同時算出要送的標籤（前 80 個）並在清單改變且超過 80 時記 warn；新增記錄成員標籤的方法（收到 heartbeat 時由 `received()` 呼叫）；`set_live` 清掉不存活成員的標籤；`summary` 的 `SummaryMember` 加 `services_off`；本機服務名稱清單由 `retain` 或 `set_off` 的呼叫端提供。
- `status.rs`：`PeerMember` 加 `#[serde(default)] services_off: Vec<String>`，`assigned_checkers` 依服務過濾候選。
- `meta/peers` 範例：`{"listen":"127.0.0.1:5302","members":[{"addr":"127.0.0.1:5303","live":true,"services_off":["api.svc.emfor"]}],"digest_mismatch":[],"disagreements":[],"services_off":[]}`。

**失敗模式：**

- `off` 格式錯誤：整則 heartbeat 以 `malformed` 丟棄、計入 `emfor_peer_messages_rejected_total{reason="malformed"}`，依既有規則每 60 秒最多 warn 一次。
- 舊版成員：忽略 `off`，行為與現在相同；新版收到舊版 heartbeat 時視為沒有關閉的服務。
- 超過 80 個標籤：多出的不送，記一行 warn，不影響其他行為。

**驗收：**

- 單元測試涵蓋 peer-group 範例「service tags」與「heartbeat `off` field」各列、「A member announces a service left out」的負責主機、80 個標籤的最壞情況長度 ≤ 1200。
- 單元測試涵蓋 status-command 範例「members' services_off and voted backends」各列與情境「Members that leave a service out are not candidates」的 JSON。
- 整合測試（crates/emfor-daemon/tests/peers.rs）涵蓋 state-store 情境「Services another member leaves out」與 peer-group 情境「A service left out on one member」的新敘述。
- 每項新行為都要確認：暫時拿掉該行為時對應測試失敗，恢復後通過。
- macOS 與 Debian 映像的 fmt、clippy、test 通過，Cargo.lock 不變。

**範圍：**

- 範圍內：heartbeat `off` 的收送與驗證、候選排除、`meta/peers` 成員欄位、status 計算、手冊（en 與 zh_TW）、examples/emfor.toml 註解、CHANGELOG。
- 範圍外：新的訊息種類、rendezvous 演算法、`down` 訊息處理、指標新增、embark.systems 文章更新。

## Risks / Trade-offs

- [新舊版混用時兩邊選出的負責主機不同] → 文件寫明需升級所有成員；混用期間的行為不比現在差（舊版本來就把該主機算作候選）。
- [服務標籤碰撞讓成員在另一個服務也被排除] → 機率低（計算值見上）；後果只是該後端少一台候選，與成員不存活相同，不會造成錯誤判斷。
- [所有成員都關閉同一服務時候選不足 3 台] → 該後端沒有負責主機、各自依本機檢查，這是預期結果，與成員不足 3 台相同。
- [`off` 讓 heartbeat 變大] → 只有設了 `peers = false` 的主機才帶；80 個標籤約 880 bytes，仍在一個 datagram 內。

## Migration Plan

升級順序不限。全部成員升級後，「同一個服務在每台主機設成一樣」由要求改為建議。回退到舊版時，舊版忽略 `off`，回到現在的行為，不需清理資料；`meta/peers` 多出的欄位會在舊版 daemon 啟動時隨整個 key 重寫。
