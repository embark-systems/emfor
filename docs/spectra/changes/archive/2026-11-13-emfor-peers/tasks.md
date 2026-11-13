## 1. 設定與檢查定義摘要

- [x] 1.1 實作 Peer group configuration：crates/emfor-daemon/src/config.rs 解析 `[peers]`（`listen`、`members`、`secret_file`，見 design「`[peers]` 設定：同一份成員清單、listen 必須是其中之一」），`Config` 多一個 `peers: Option<PeersConfig>`；依 Config validation rules 把 spec peer-group「peer config validation」表格每一列與 service-config 新增的兩列都回報為帶 TOML 路徑的錯誤，並與其他錯誤一起列出。驗證：config.rs 單元測試逐列對照表格（密鑰檔用 tempfile 建立並設定 mode 0600 / 0644），以及沒有 `[peers]` 時 `Config::peers` 為 `None`。
- [x] 1.2 [after: 1.1] 實作 Check definition digest（design「檢查定義摘要」）：`ServiceConfig` 多一個 `digest` 欄位，在設定載入時以 `checker_id`、`check_options`（所有表格鍵依字母排序）與 `timeout` 毫秒算出 32 個小寫 hex。驗證：config.rs 單元測試涵蓋 spec「what changes the digest」表格每一列，以及 `check_options` 鍵順序不同摘要相同。

## 2. 聯防模組：訊息、認證、密鑰、心跳

- [x] 2.1 新增 crates/emfor-daemon/src/peers.rs 的訊息編解碼（design「UDP 訊息格式：魔數、HMAC 標籤、JSON 內容」）：`EMP1` + HMAC-SHA256 + JSON，`heartbeat` 與 `down` 兩種 `kind`，`reason` 在字元邊界截到 256 bytes，datagram 不超過 1200 bytes；crates/emfor-daemon/Cargo.toml 加 `hmac = "0.13"` 與 `sha2 = "0.11"`。驗證：單元測試編碼後解碼相等、多位元組字元的 reason 截斷、`git diff Cargo.lock` 只有 emfor-daemon 的相依清單變動、沒有新增套件。
- [x] 2.2 [after: 2.1] 實作 Peer message authentication and replay protection（design「訊息認證與防重送」）：依序檢查 `malformed`、`auth`、`sender`、`site`、`stale`、`replay`，HMAC 用常數時間比較；每個傳送者記錄 `(boot, seq, ts)`；不認得的 `kind` 只算存活；被拒時每來源位址每原因每 60 秒最多一行 warn。驗證：單元測試涵蓋 spec「acceptance checks」表格每一列與「Sender restarts」情境，warn 限流以 paused time 測試。
- [x] 2.3 [after: 2.2] 實作 Peer secret reload（design「密鑰檔每秒最多重讀一次，換密鑰時舊密鑰再接受 60 秒」）：每輪心跳前檢查 `secret_file`，內容變了且有效就換成目前密鑰、前一把再接受 60 秒；讀不到或內容不合規則時沿用最後一把有效密鑰並每分鐘最多一行 warn。驗證：單元測試以 paused time 驗證換密鑰後 30 秒仍接受舊密鑰、90 秒拒絕，以及刪除檔案後仍以舊密鑰送出。
- [x] 2.4 [after: 2.3] 實作 Peer heartbeats and liveness（design「每秒心跳，3 秒內收到有效訊息即為存活」與「聯防模組是一個 tokio task，`[peers]` listen 變更需重啟」）：聯防 task 綁定 `listen`、每秒對每個其他成員送 `heartbeat`、3 秒內收到有效訊息為存活、存活變化記 info；`members` 與 `secret_file` 透過 `watch` 更新，移除的成員連同防重送記錄一起刪除、新增的成員從未存活開始。驗證：單元測試在 127.0.0.1 兩個 socket 之間互送，確認對方 1 秒內變存活、停掉一方後 4 秒內變未存活、更新 `members` 後新成員開始收到心跳。

## 3. 立刻重查與故障通知

- [x] 3.1 實作立刻重查通道與快節奏連查（design「立刻重查通道」與「故障快速通知與快節奏連查」，修改 Check scheduling with jitter and no overlap）：crates/emfor-daemon/src/schedule.rs 的 `spawn_backend` 多收一個 `Arc<Notify>` 與一個可選的通知 sender；等待以 `select!` 同時等 sleep 與 `notified()`；收到時只有在 `healthy`、不在連查中、距上段連查結束超過 `fall × interval` 才開始連查，連查以 `min(interval, 200ms)` 間隔、不加 jitter，遇到 `up`、轉為 `unhealthy` 或滿 `fall` 次停止；`healthy` → `unhealthy` 時經 sender 交出（服務、後端、原因），`unknown` → `unhealthy` 不交出。驗證：schedule.rs 以 paused time 的單元測試涵蓋 spec「Fast recheck run after a peer report」情境、「whether a received down message starts a fast recheck run」表格每一列、`unknown` → `unhealthy` 不交出，且現有 schedule.rs 測試只需補上新參數即通過。
- [x] 3.2 [after: 1.2, 3.1] 在 crates/emfor-daemon/src/supervisor.rs 維護登記表（服務名稱 → 摘要、後端 id → `Notify`）：`Supervisor::sync` 建立 task 時登記、停止時移除，摘要隨重載更新；沒有 `[peers]` 時不傳通知 sender。驗證：supervisor.rs 單元測試確認重載移除後端後登記表不再有它、`check_options` 變更後摘要更新。
- [x] 3.3 [after: 2.4, 3.2] 接起 Backend failure notification：聯防 task 把後端 task 交出的轉換以 `down` 送給每個其他成員；收到並通過驗證的 `down` 依登記表找到服務、比對摘要與後端後喚醒該後端的 `Notify`，摘要不同或找不到時記 debug 並忽略；開始連查時記 spec 規定的 info 行。驗證：peers.rs 單元測試以兩個聯防 task 與假的登記表確認通知送達會喚醒對應的 `Notify`、摘要不同時不喚醒，且通知本身不產生任何 `WriterMsg`。

## 4. 啟動、重載、指標與整合測試

- [x] 4.1 [after: 3.3] 在 crates/emfor-daemon/src/cli.rs 的 `run` 啟動聯防 task（綁定失敗記 error 並以 `EXIT_STARTUP` 結束），並實作 Peer listen requires restart 與修改後的 Reload on SIGHUP applies only a fully valid config：`[peers] listen` 變更或 `[peers]` 新增 / 移除為重載錯誤 `peers listen settings require restart`，`members` 與 `secret_file` 於重載生效。驗證：crates/emfor-daemon/tests/daemon.rs 新增測試涵蓋 service-config「Peers listen change requires restart」情境與綁定已占用埠時啟動失敗。
- [x] 4.2 [after: 4.1] 實作修改後的 Daemon metrics（design「指標」）：crates/emfor-daemon/src/live.rs 保存聯防狀態，crates/emfor-daemon/src/metrics.rs 輸出 `emfor_peer_up`、`emfor_peer_messages_rejected_total`（六個 reason 從 0 開始）、`emfor_peer_notifications_total`；沒有 `[peers]` 時不輸出任何 `emfor_peer_` 序列；重載移除成員後其 `emfor_peer_up` 序列消失。驗證：metrics.rs 單元測試涵蓋 metrics-endpoint「Peer metrics」情境與沒有 `[peers]` 的情況。
- [x] 4.3 [after: 4.2] 新增 crates/emfor-daemon/tests/peers.rs 整合測試：兩個 daemon 在 127.0.0.1 不同埠組成群組，檢查同一個由測試控制的 TCP 後端（`interval = "5s"`、`fall = 3`），涵蓋 spec「Peer sees the failure first」（第二台在第一台判定後 2 秒內判定 `unhealthy`）、「Member stops」（停掉一台後 4 秒內 `emfor_peer_up` 為 0）、「Member added on reload」與「Digest differs」。驗證：`cargo test -p emfor-daemon --all-features --test peers` 通過，並在拿掉通知送出後確認「Peer sees the failure first」失敗。

## 5. 文件與最終檢查

- [x] 5.1 文件寫出聯防群組與三階段完整目標：改寫 docs/manual/01-concepts.md「no communication between emfor daemons」那句（改為同一個 `site` 可選擇組成聯防群組、查詢仍只讀本機 store）並把 `emfor-check` 那列補齊為 redis、MySQL/MariaDB、PostgreSQL、LDAP、DNS、RADIUS、SIP、Diameter；docs/manual/03-configuration.md 新增 `[peers]` 一節並在「What a reload changes」加上重載規則；docs/manual/08-operations.md 的 Metrics 與 Logs 加上新指標與 log 行、說明 NTP 與 UDP 埠需求；docs/deploy/daemon.md 加上部署步驟（design「Migration Plan」）；examples/emfor.toml 加註解掉的 `[peers]` 範例；CHANGELOG.md 加一條；docs/spectra/config.yaml 專案說明的「第一版不做：多台 emfor 同步」改為聯防群組三階段的現況。驗證：逐檔內容檢查與 design 一致，且 `emfor check-config --config examples/emfor.toml` 仍回報 config ok。
- [x] 5.2 [after: 4.3, 5.1] 最終檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過；`git diff Cargo.lock` 沒有新增套件；沒有 `[peers]` 的 health-checking 與 metrics 既有測試未修改斷言即通過。驗證：上述指令的輸出。

## 6. 驗證與審查後的修正

說明（非任務）：任務 4.1 的兩個測試實際放在 crates/emfor-daemon/tests/peers.rs（`reload_rejects_peers_listen_change`、`occupied_peer_port_exits_1`），不在 tests/daemon.rs；任務 4.3 的「Peer sees the failure first」依修改後的規格情境以 A `interval = "1s"`、B `interval = "30s"` 驗證，拿掉通知送出時該測試失敗（B 未在 2 秒內跟上）。

- [x] 6.1 依 design「密鑰檔每秒最多重讀一次，換密鑰後舊密鑰再簽 60 秒、再接受 120 秒」修正密鑰輪替：`Keys::signing` 在換密鑰後 60 秒內回傳舊密鑰、`Group::encode` 以它簽章，舊密鑰接受期改為 120 秒；文件（docs/manual/03-configuration.md、docs/deploy/daemon.md）同步。驗證：peers.rs 的 `secret_rotated_on_one_host_first`（A 第 0 秒、B 第 40 秒換，0–200 秒雙向每則都接受；改回用新密鑰簽章時此測試失敗）與 `previous_secret_accepted_for_120_seconds`（30、90 秒接受，130 秒拒絕）。
- [x] 6.2 Peer group configuration 補上位址類型規則：`members` 中與 `listen` 不同位址類型（IPv4 / IPv6）的成員回報 `peers.members[<i>]` 錯誤；重複成員的錯誤路徑在規格中改為 `peers.members[<i>]`。驗證：config.rs `peer_config_validation` 表格新增混用 IPv4 / IPv6 一列並通過。
- [x] 6.3 [after: 6.1] 補齊測試：tests/peers.rs 的 `reload_rejects_peers_listen_change` 改為由 B 的位址送一則簽章過的心跳，確認 `emfor_peer_up{peer="<B>"} 1`（舊位址仍在接收）；schedule.rs 新增 `run_stops_after_fall_checks`（每次都回 `error`，做滿 `fall` 次後回到 `interval ± jitter`）。驗證：兩個測試通過。
- [x] 6.4 [after: 6.1, 6.2, 6.3] 重跑最終檢查：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -D warnings`、`cargo test --workspace --all-features` 在 macOS 與 Debian 映像 emfor-clients-test 通過，`emfor check-config --config examples/emfor.toml` 回報 config ok。驗證：上述指令的輸出。
