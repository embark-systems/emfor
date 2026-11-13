## Why

目前每台主機的 emfor daemon 各自檢查、互不溝通（docs/manual/01-concepts.md）：同一個機房裡十台應用主機會各自花 `fall × interval` 才發現同一台後端壞了，也各自對後端送一份檢查。使用者決定加入「聯防群組」（LANGUAGE.md）：同一個 `site` 的 daemon 互相交換檢查結果，分三個 change 完成——更快發現故障、多數決避免誤判、分工減少檢查量。這個 change 是第一階段：先建好成員、認證與交換，並用它讓故障更快傳開。

## What Changes

- 新增設定檔 `[peers]` 區段：`listen`（本機 UDP 位址，預設埠 5302）、`members`（同一個 `site` 所有成員的位址，含本機，每台主機可用同一份清單）、`secret_file`（HMAC 密鑰檔）。有 `[peers]` 時最上層 `site` 為必填。沒有 `[peers]` 時行為與現在完全相同。
- 成員之間以 UDP 每秒互送心跳；3 秒內收到某成員通過驗證的訊息即視為存活。存活變化記一行 log。
- 每則訊息以 HMAC-SHA256 認證（密鑰取自 `secret_file`，每秒最多重讀一次，換密鑰後 60 秒內仍以舊密鑰簽章、120 秒內仍接受舊密鑰），帶傳送時間、啟動識別碼與序號；時間差超過 5 秒、序號沒有前進、來源位址與 `from` 不符、不是成員的訊息一律丟棄並計數。
- 每個服務算出「檢查定義摘要」（checker、`check_options`、`timeout` 的 SHA-256）；訊息附這個摘要，摘要不同的成員不在該服務聯防。
- 故障快速通知：某台 daemon 看到後端由 `healthy` 變 `unhealthy` 時，立刻通知所有成員；收到的成員若該後端仍是 `healthy`、摘要相同，立刻重查，並以快節奏（`min(interval, 200ms)`）連查到後端轉為 `unhealthy`、出現一次 `up` 或查滿 `fall` 次為止。通知只讓本機更快重查，判斷仍完全來自本機自己的檢查結果。
- 每個後端的檢查 task 加上「立刻重查」通道。
- `/metrics` 新增 `emfor_peer_up{peer}` 與 `emfor_peer_messages_rejected_total{reason}`、`emfor_peer_notifications_total{direction}`。
- 重載：`members`、`secret_file` 變更於 SIGHUP 生效；`[peers] listen` 變更（含新增或移除 `[peers]`）需重啟。
- 文件：改寫 docs/manual/01-concepts.md「daemon 之間不溝通」那句並補齊 `emfor-check` 外掛清單；設定手冊、部署文件、範例設定檔與 changelog 加上 `[peers]`。
- design.md 寫出三階段完整目標，後兩個 change（`emfor-peers-quorum`、`emfor-peers-split-checks`）照著做。

## Non-Goals (optional)

見 design.md 的 Goals / Non-Goals。

## Capabilities

### New Capabilities

- `peer-group`: 聯防群組的成員、存活判斷、訊息認證與防重送、檢查定義摘要，以及故障快速通知與收到後的重查。

### Modified Capabilities

- `service-config`: 設定驗證規則加入 `[peers]` 與「有 `[peers]` 時 `site` 必填」；重載規則加入 `[peers] listen` 需重啟、`members` 與 `secret_file` 可重載。
- `health-checking`: 檢查排程加入例外——收到成員的故障通知時立刻重查並以快節奏連查。
- `metrics-endpoint`: daemon 指標加入聯防群組的成員存活、被拒訊息與通知計數。

## Impact

- 程式碼：crates/emfor-daemon/src/config.rs（`[peers]` 解析與驗證、檢查定義摘要）、crates/emfor-daemon/src/schedule.rs（立刻重查通道與快節奏連查、轉為 unhealthy 時送出通知）、crates/emfor-daemon/src/supervisor.rs（建立並登記每個後端的重查通道）、新增 crates/emfor-daemon/src/peers.rs（UDP、訊息編解碼、HMAC、防重送、成員存活）、crates/emfor-daemon/src/cli.rs（啟動與重載）、crates/emfor-daemon/src/live.rs 與 crates/emfor-daemon/src/metrics.rs（指標）、crates/emfor-daemon/src/lib.rs。
- 測試：crates/emfor-daemon/tests/peers.rs（兩個 daemon 在本機互相通知）。
- 相依：emfor-daemon 加 hmac 0.13 與 sha2 0.11；兩者已在 Cargo.lock（emfor-check 與 postgres-protocol 使用），Cargo.lock 不新增套件。
- 文件：docs/manual/01-concepts.md、docs/manual/03-configuration.md、docs/manual/08-operations.md、docs/deploy/daemon.md、examples/emfor.toml、CHANGELOG.md、docs/spectra/config.yaml 的專案說明（「第一版不做：多台 emfor 同步」改寫）。
- 網路：有 `[peers]` 的主機之間需開放 UDP 5302（或自訂埠）。LMDB 格式、client、DNS 不變。
