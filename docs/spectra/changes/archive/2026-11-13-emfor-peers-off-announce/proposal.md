## Why

服務設了 `peers = false` 的主機不送 `result`，但其他成員不知道，仍會把它選為該服務後端的負責主機之一；結果那些後端實際上只有兩台負責主機會投票，任一台缺票就無法投票決定，分工省下的檢查也跟著變少。目前只能靠文件要求「同一個服務在每台主機都設成一樣」，有主機刻意只讓自己不參加時就沒有辦法。

## What Changes

- 每個成員在 heartbeat 加上選填欄位 `off`：自己 `peers = false` 的服務名稱的短雜湊（UTF-8 名稱的 SHA-256 前 4 bytes，寫成 8 個小寫十六進位字元），依字串排序、去重，最多 80 個；沒有關閉的服務時不帶這個欄位。超過 80 個時只送排序後的前 80 個，啟動與每次 reload 改變清單時記一行 `warn`，沒送出的那些服務仍會被其他成員算作候選。
- 收到 heartbeat 時記下該成員最新的 `off` 集合（沒有欄位即為空），成員不再存活時清空。`off` 不是字串陣列、或有元素不是 8 個小寫十六進位字元的 heartbeat 以 `malformed` 丟棄。
- 選服務 S 的後端負責主機時，候選成員排除 `off` 集合含有 S 雜湊的存活成員；候選不到 3 台時該後端沒有負責主機（規則不變）。
- `meta/peers` 的 `members` 每一項多 `services_off`：本機服務中雜湊落在該成員 `off` 集合裡的名稱，依名稱排序；`emfor status` 計算 `voted` 與 `assigned_checkers` 時同樣排除這些成員，缺欄位視為空。
- 舊版成員忽略 `off` 欄位，照舊把該主機算作候選；文件改為「升級所有成員後，各主機可以各自決定哪些服務不參加」。

## Non-Goals

（見 design.md 的 Goals / Non-Goals。）

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `peer-group`: heartbeat 多 `off` 欄位與其驗證；負責主機的候選排除宣告不參加該服務的成員；「Services left out of the peer group」改為會告知其他成員。
- `state-store`: `meta/peers` 的 `members` 每一項多 `services_off`。
- `status-command`: `voted` 與 `assigned_checkers` 的計算排除各成員 `services_off` 裡的服務。

## Impact

- 程式：crates/emfor-daemon/src/peers.rs（`Wire.off`、heartbeat 編碼與驗證、收到 heartbeat 時交給 quorum）、crates/emfor-daemon/src/quorum.rs（各成員的關閉集合、`assigned` 的候選排除、`summary` 的 `members[].services_off`）、crates/emfor-daemon/src/supervisor.rs（把本機服務名稱與關閉清單交給 peer task）、crates/emfor-daemon/src/status.rs（`PeerMember.services_off` 與負責主機計算）。
- 測試：crates/emfor-daemon/tests/peers.rs 整合測試，以及上述模組的單元測試。
- 文件：docs/manual/01-concepts.md、docs/manual/03-configuration.md、docs/manual/08-operations.md 及 docs/manual.zh_TW 的對應章節、examples/emfor.toml 註解、CHANGELOG.md。
- 相依套件：不新增（sha2 已在 crates/emfor-daemon/Cargo.toml）。
- 相容性：訊息格式仍為 `EMP1`，`meta/schema_version` 仍為 `1`；新舊版混用時，舊版成員選出的負責主機可能與新版不同，直到全部升級。
