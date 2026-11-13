## 1. 送出與接收服務標籤

- [x] 1.1 crates/emfor-daemon/src/quorum.rs 加服務標籤函式（UTF-8 名稱 SHA-256 前 4 bytes，big-endian `u32`，寫成 8 個小寫十六進位字元），`Quorum::set_off` 同時保存去重排序後的前 80 個標籤，清單改變且超過 80 個時記一行 `warn` 說明幾個沒送出（design「每則 heartbeat 最多 80 個服務標籤」）。依 peer-group「Peer heartbeats and liveness」。驗證：quorum 單元測試重現範例「service tags」三列（`api.svc.emfor` → `a513a047`、`db.svc.emfor` → `fa6f63b4`、`web.svc.emfor` → `99e6433f`），81 個標籤只保留最小的 80 個。
- [x] 1.2 crates/emfor-daemon/src/peers.rs 的 `Wire` 加 `off`（沒有標籤時不序列化），heartbeat 編碼時帶 1.1 保存的標籤；`decode` 對 `off` 存在但不是 8 個小寫十六進位字元字串陣列的 heartbeat 回 `malformed`（design「以 heartbeat 的 off 欄位傳送服務標籤」）。依 peer-group「Peer heartbeats and liveness」。驗證：peers 單元測試涵蓋範例「heartbeat `off` field」七列；以最長 `from`（IPv6 含 scope id）、63 字元 `site`、`seq = u64::MAX` 與 80 個標籤編碼的 heartbeat 不超過 `MAX_DATAGRAM`；沒有關閉服務時 heartbeat 的 JSON 不含 `off`。[after: 1.1]
- [x] 1.3 收到 heartbeat 時 `received()` 以其 `off` 取代該成員保存的標籤（沒有欄位即清空），`down` / `result` / 未知種類不改變，`Quorum::set_live` 清掉不存活成員的標籤（design「成員的服務標籤由最新 heartbeat 取代並在不存活時清空」）。依 peer-group「Peer heartbeats and liveness」情境「Kept tags follow the latest heartbeat」。驗證：quorum 或 peers 單元測試依序送 heartbeat（`off` 含 `a513a047`）、`result`、無 `off` 的 heartbeat，檢查每一步保存的標籤；成員不再存活後標籤為空；暫時拿掉取代邏輯時測試失敗。[after: 1.2]

## 2. 負責主機與 meta/peers

- [x] 2.1 `Quorum::combine` 與 `Quorum::votes` 呼叫 `assigned` 前，把保存標籤含有該服務標籤的存活成員從候選拿掉，`assigned` 簽名不變（design「候選排除在呼叫 assigned 之前完成」）。依 peer-group「Assigned checkers and check results」與「Services left out of the peer group」。驗證：quorum 單元測試重現情境「A member announces a service left out」（`10.0.0.11:8080` 變成 .5、.6、.7，`10.0.0.12:8080` 仍是 .9、.6、.7），以及兩個本機服務標籤相同時兩者都排除該成員；既有 `assigned_checkers` 測試不改仍通過；暫時拿掉排除時新測試失敗。[after: 1.3]
- [x] 2.2 `Quorum::summary` 的每個成員輸出 `services_off`：本機服務（crates/emfor-daemon/src/supervisor.rs 的 `publish_targets` 交給 quorum 的參加與不參加聯防的服務名稱）中標籤落在該成員保存標籤裡的名稱，依名稱排序（design「meta/peers 的成員多 services_off 名稱」）。依 state-store「Peer group state in the store」。驗證：quorum 單元測試的 `summary` 期望字串加上 `"services_off":[]` 與一個非空的例子（本機沒有的服務的標籤不出現）；crates/emfor-daemon/src/writer.rs 測試裡寫死的 `meta/peers` 值同步更新後通過（status.rs 測試的成員沒有 `services_off` 也照舊通過）；state-store 情境「Peer state written at start」的啟動值成立。[after: 2.1]
- [x] 2.3 整合測試 crates/emfor-daemon/tests/peers.rs：(a) 一個成員宣告本機服務 `api.svc.emfor` 與本機沒有的 `web.svc.emfor`，2 秒內 `meta/peers` 的該成員 `services_off` 為 `["api.svc.emfor"]`，該成員停止後 2 秒內為 `[]`（state-store 情境「Services another member leaves out」）；(b) 三個 daemon、其中 A 對 `api.svc.emfor` 設 `peers = false`，另外兩個不把 A 算作候選、後端沒有負責主機並依自己的檢查標成 `unhealthy`，A 保持 `healthy`（peer-group 情境「A service left out on one member」）。驗證：`cargo test -p emfor-daemon --test peers` 通過；暫時讓 heartbeat 不帶 `off` 時 (a) 失敗。[after: 2.2]

## 3. status

- [x] 3.1 crates/emfor-daemon/src/status.rs 的 `PeerMember` 加 `services_off`（缺少時為空），`assigned_checkers` 依服務把 `services_off` 含該服務的存活成員從候選拿掉，影響 `voted`、`checks n/m backends here` 與 `--json` 的 `assigned_checkers`。依 status-command「Status text output」。驗證：新單元測試重現範例「members' services_off and voted backends」四列與情境「Members that leave a service out are not candidates」的 JSON；既有 `voted_backends_table`、`json_assigned_checkers` 不改仍通過（成員缺欄位時照舊）；暫時拿掉過濾時新測試失敗。[after: 2.2]

## 4. 文件與完整檢查

- [x] 4.1 文件改成「其他成員會被告知、升級所有成員後各主機可各自決定」：docs/manual/03-configuration.md 與 docs/manual.zh_TW/03-configuration.md 的 `peers = false` 說明、docs/manual/01-concepts.md 與 docs/manual.zh_TW/01-concepts.md 的聯防群組一節、docs/manual/08-operations.md 與 docs/manual.zh_TW/08-operations.md 的 `meta/peers`（成員的 `services_off`）、examples/emfor.toml 註解、CHANGELOG.md 新條目（含 80 個上限與舊版相容說明）。驗證：人工審閱與三份 spec 一致；crates/emfor-daemon/tests/daemon.rs 中驗證 examples/emfor.toml 的既有測試通過。[after: 2.3, 3.1]
- [x] 4.2 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 4.1]

## 5. 驗證與審查後的修正

- [x] 5.1 crates/emfor-daemon/src/quorum.rs 的 `Quorum::set_off` 警告依數量寫成「1 tag is not sent」或「N tags are not sent」；整合測試 crates/emfor-daemon/tests/peers.rs 以 81 個 `peers = false` 的服務啟動 daemon，log 剛好有一行含 `1 tag is not sent` 的 `warn`，用同一份設定 reload 後仍只有一行（peer-group 情境「More services left out than fit」）。驗證：`cargo test -p emfor-daemon --test peers` 通過；暫時拿掉警告時測試失敗。
- [x] 5.2 整合測試 crates/emfor-daemon/tests/peers.rs：以測試用 UDP socket 當另一個成員接收 daemon 的 heartbeat，`api.svc.emfor`、`db.svc.emfor` 設 `peers = false`、`web.svc.emfor` 為 `true` 時 payload 帶 `"off":["a513a047","fa6f63b4"]`；reload 讓三者都是 `true` 後，之後的 heartbeat 不帶 `off`（peer-group 情境「Heartbeat announces services left out」）。驗證：`cargo test -p emfor-daemon --test peers` 通過；暫時讓 heartbeat 不帶標籤時測試失敗。
- [x] 5.3 完整檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，Cargo.lock 沒有變更。驗證：三個指令結束碼為 0 並記錄測試套件數。[after: 5.1, 5.2]
