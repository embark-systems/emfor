# 11. 聯防群組

[手冊](README.md) · 上一章：[10. DNS](10-dns.md) · 下一章：[12. 維運 emfor](12-operating.md)

沒有聯防群組時，每個 daemon 都自己決定；這很單純，也撐得過任何網路故障，但有兩個代價：每台主機都需要自己的 `fall` 次檢查失敗才會發現故障，而且每台主機都要檢查每個 backend。由同一個資料中心的 daemon 組成的**聯防群組**（peer group）分擔這些工作：

- **故障通知。** 某個成員看到 backend 從 `healthy` 變成 `unhealthy` 時，會通知其他成員。其他每個成員，如果那個 backend 在它那裡是 `healthy`，就開始一輪**快速重查**：每 200 ms 檢查一次（`interval` 較短時則每個 `interval` 一次），直到有一次檢查通過、backend 變成 `unhealthy`，或已經檢查了 `fall` 次。所以整個群組在大約 `fall × 200 ms` 之內就會跟上第一個成員。
- **投票。** 存活成員有三台以上時，其中三台是每個 backend 的**負責主機**（assigned checker）。三台中有兩台看到它 down 時，它在每個成員上都算 down，不論該成員自己的檢查怎麼說。
- **較少的檢查。** 不是某個 backend 負責主機的成員，可以採用投票的結果，而不必自己檢查。

查詢與 DNS 仍然只讀本機的 daemon。連不到其他成員的成員，和沒有群組時一樣，依自己的檢查決定。

## 設定

每個成員都有一個 `[peers]` table，以及一個 `site`：

```toml
site = "dc1"

[peers]
listen = "10.0.1.11:5302"
members = ["10.0.1.11:5302", "10.0.1.12:5302", "10.0.1.13:5302"]
secret_file = "/etc/emfor/peers.key"

[services."api.svc.emfor"]
check = "http"
backends = ["10.0.2.21:8080", "10.0.2.22:8080"]
```

| 鍵 | 預設值 | 規則 |
| --- | --- | --- |
| `listen` | 必填 | 這台主機的 `ip:port`（UDP），不能是 `0.0.0.0` 或 `::`。改變它，或加入、移除 `[peers]`，需要重新啟動。 |
| `members` | 必填 | 2–64 個不重複的位址，全都與 `listen` 同一個位址族，且包含 `listen`。可以 reload。 |
| `secret_file` | 必填 | 共用的密鑰（[見下文](#密鑰)）。可以 reload，也會監看它的變化。 |
| `previous_secret_file` | 無 | 只在啟動時讀取，用於更換密鑰途中的重新啟動（[見下文](#更換密鑰)）。 |
| `encrypt` | `true` | 加密訊息。`false` 只簽署訊息。可以 reload。 |
| `accept_plaintext` | `encrypt` 為 `true` 時是 `false`，否則是 `true` | 接受經過簽署但未加密的訊息。`false` 與 `encrypt = false` 一起設定是錯誤。可以 reload。 |

有 `[peers]` 時 `site` 是必填的。群組的成員應該在同一個資料中心：投票的前提是，一個成員看到的，其他成員也會看到。

所有成員都應該以相同的檢查定義列出相同的服務。成員之間比對每個服務檢查的**摘要**（digest）（檢查名稱，或 plugin 及它的 command 與 `env_pass`、`check_options` 與 `timeout`）；成員會忽略摘要不同的服務的票與通知，`emfor status` 會顯示摘要不符。`interval`、`rise`、`fall`、`balance`、`on_all_down` 與 backend 清單可以不同。

在成員之間開放這個 UDP port。每則訊息是一個最多 1200 位元組的 datagram；每個成員每秒對其他每個成員送一次 heartbeat。

### 讓某個服務不參加

在服務上（或在 `[defaults]` 中）設定 `peers = false`，讓這個服務在這台主機上不參加群組：它在這裡每個 interval 都會被檢查，結果不會送出，它的票與通知都會被忽略。這台主機會在 heartbeat 中告訴其他成員，所以它們不會把它算成那個服務的負責主機。

## 投票如何運作

對每個 backend，每個成員把候選（自己，以及每個沒有讓該服務不參加的存活成員）依 `<service>/<backend>/<member listen>` 的 MD5 雜湊排名，取前三名作為負責主機。每個成員都算出同樣的三台，不需要互相溝通。候選少於三台時就沒有負責主機，每個成員各自檢查。

每次檢查之後，成員計算這三台最新的結果（結果必須夠新，在 `3 × interval` 之內，來自存活的成員，且摘要相同，才計入）：

| 票 | 套用的結果 |
| --- | --- |
| 沒有負責主機，或沒有票 | 這個成員自己的結果 |
| 2 票以上 `down` | `down`，reason 為 `peers <d>/<f>: <reason of the first down vote>` |
| 其他情況 | 這個成員自己的結果；`down` 的 reason 為 `local: <reason>` |

所以自己的檢查通過的成員，在三台負責主機中有兩台看到 backend down 時，仍然可能把它標成 down。這就是**意見不一致**（disagreement），以 `info` 記錄一次：
`backend <backend> of <service> is down at <d>/<f> peers but up here`，並計入 `emfor_peer_disagreements`。意見不一致通常表示這台主機與 backend 之間有網路問題，或檢查定義不同。

不是某個 backend 負責主機的成員，在兩張夠新的票一致時會略過自己的檢查（`emfor_peer_checks_skipped_total` 計算這些次數），但在連續略過兩輪之後、自己最近一次的結果不是 `up` 時、快速重查期間，或票不是新的時候，仍然會自己檢查。每個結果只在一次檢查時間計入，所以其他成員的結果絕不會讓 `rise` 或 `fall` 前進兩次。

`emfor status` 會顯示有多少成員存活。它不會列出負責主機是哪幾台；`emfor status --json` 會為每個 backend 列出它們（`assigned_checkers`，[第 12 章](12-operating.md#emfor-status)）。文字輸出只顯示這台主機的處境：它不是某些顯示中 backend 的負責主機時，`peers:` 那一行結尾是 `, checks <n>/<m> backends here`，那些 backend 標上 `voted`；顯示中的 backend 都沒有負責主機時，那一行結尾是 `, too few live members to vote`。三個成員的群組裡，每個成員都是每個 backend 的負責主機，所以三個都在執行時，那一行只有：

```text
peers: 2/2 live
```

三個成員中有一個停止之後：

```text
peers: 1/2 live, not live 127.0.0.3:5302, too few live members to vote
```

（計數的是**其他**成員。）一個成員在最近 3 秒內有收到它的有效訊息時就算存活。log 先寫出
`peer group on 127.0.0.1:5302 with 2 other members`，接著是
`peer 127.0.0.2:5302 is live`，之後是
`peer 127.0.0.3:5302 is not live (no valid message for 3s)`。

這些輸出來自一個 Debian 容器上的三個 daemon（`127.0.0.1`–`127.0.0.3`）；跨真實主機、經真實網路的群組沒有為本手冊試過。

## 密鑰

所有成員共用一把密鑰。訊息以它驗證，設定 `encrypt = true` 時也以它加密。建立一次，並複製到每個成員：

```sh
# head -c 32 /dev/urandom | base64 > /etc/emfor/peers.key
# chown root:emfor /etc/emfor/peers.key
# chmod 0640 /etc/emfor/peers.key
```

這個檔案必須：

- 是絕對路徑，daemon 讀得到；
- 由 root 或 daemon 的使用者擁有（群組無所謂）；
- 其他人不能讀，群組與其他人不能寫；
- 去掉一個結尾的換行之後至少有 32 位元組，全部最多 65536 位元組。

檔案的位元組就是密鑰；不必是 base64。`emfor check-config` 會拿擁有者與執行它的使用者比對：像上面那樣建立的 `root:emfor` 檔案，以 daemon 的使用者或 root 執行都會通過，而 `emfor` 擁有的檔案在由 root 執行檢查時不會通過。

### 訊息格式

| 格式 | 何時使用 | 保護 |
| --- | --- | --- |
| `EMP1` | `encrypt = false` | HMAC-SHA256：經過驗證，在網路上可被讀取。 |
| `EMP2` | `encrypt = true`，送給還不知道能讀 `EMP3` 的成員（0.6 與更早版本） | ChaCha20-Poly1305 |
| `EMP3` | `encrypt = true`，送給送過 `EMP3` 或宣告過支援它的成員 | XChaCha20-Poly1305 |

成員一開始對所有人送 `EMP2`，一旦某個成員的訊息顯示它能讀 `EMP3`，就對那個成員改送 `EMP3`；log 會寫出
`sending EMP3 to peer 127.0.0.2:5302 from now on`。你不需要設定任何東西。

每則訊息都帶有時間戳記、boot id 與序號；重送的、太舊的、格式錯誤的或簽章錯誤的訊息，來自不在 `members` 中的位址的、來自另一個 site 的，以及（預設）未加密的訊息，都會被丟棄，並以 `replay`、`stale`、`malformed`、`auth`、`sender`、`site` 或 `plaintext` 計入 `emfor_peer_messages_rejected_total{reason}`。

## 更換密鑰

daemon 會自己察覺 `secret_file` 的變化（Linux 上用 inotify，FreeBSD 與 macOS 上用 kqueue，而且無論如何每 30 秒讀一次）。不需要 reload。變化之後它：

- 繼續以舊的密鑰**簽署** 60 秒；
- 繼續**接受**舊的密鑰 120 秒。

所以：**在 60 秒內**把新的密鑰寫到每個成員上，就不會遺失任何訊息。

```sh
# head -c 32 /dev/urandom | base64 > /tmp/peers.key.new
# for h in host1 host2 host3; do
>   scp /tmp/peers.key.new $h:/etc/emfor/peers.key.new &&
>   ssh $h 'chown root:emfor /etc/emfor/peers.key.new && chmod 0640 /etc/emfor/peers.key.new &&
>           mv /etc/emfor/peers.key.new /etc/emfor/peers.key'
> done
```

用 `mv`（rename）取代檔案，daemon 就絕不會讀到半個檔案。讀不到或無效的檔案會被忽略：daemon 保留最後一把有效的密鑰，並且最多每分鐘警告一次。請留意
`emfor_peer_messages_rejected_total{reason="auth"}`：成員使用不同的密鑰時它會上升。

### 更換途中重新啟動

在更換途中重新啟動的 daemon 只知道新的密鑰，會丟棄仍以舊密鑰簽署的成員送來的訊息，直到它們切換為止。要銜接這段時間，把舊的密鑰保留在 `previous_secret_file`：

```toml
site = "dc1"

[peers]
listen = "10.0.1.11:5302"
members = ["10.0.1.11:5302", "10.0.1.12:5302", "10.0.1.13:5302"]
secret_file = "/etc/emfor/peers.key"
previous_secret_file = "/etc/emfor/peers.key.old"
```

`previous_secret_file` 只在下列條件全部成立時才會使用：daemon 的記錄
`<store>-peers`（`/var/lib/emfor/state.mdb-peers`，權限 0600）表示被替換的正是那把密鑰，而且是在 120 秒以內。這份記錄存放的是 SHA-256 指紋與時間，從不存放密鑰。所以舊的密鑰不可能因為某個檔案還留著它，就在之後又回來。在**從 0.6 升級後第一次啟動**時還沒有這份記錄，`previous_secret_file` 會被忽略，並記錄一行 `info`。

## 從雲端 API 取得成員

`members` 可以用 reload 改變，所以主機會來來去去的群組可以用 timer 保持最新。
[examples/deploy/emfor-peers-sync.sh](../../examples/deploy/emfor-peers-sync.sh)
從標準輸入取得成員清單，把它寫到一個只含 `[peers] members` 的 `conf.d` 檔案，以 daemon 的使用者執行 `emfor check-config`，失敗時把舊檔放回去，然後 reload：

```sh
# some-command-that-lists-the-hosts | /usr/local/sbin/emfor-peers-sync.sh /etc/emfor/conf.d/50-peers.toml
```

清單少於 2 個或多於 64 個項目（通常是 API 回了空的回答），或有一行不是位址時，它會保留舊的檔案。`[peers]` 的其餘部分留在主設定檔中。由 reload 加入的成員一開始是不存活的；被移除的成員會被遺忘。CHANGELOG 記載這支腳本曾在測試容器中執行過；它沒有為本手冊執行過。

## 從 0.6 升級群組

- 0.7 預設拒絕未加密的（`EMP1`）訊息。如果你的 0.6 群組以 `encrypt = false` 執行，請在所有成員上開啟加密，或在 0.7 的成員上設定 `accept_plaintext = true`，直到每個成員都升級完成。
- 密鑰檔現在必須由 root 或 daemon 的使用者擁有。
- 0.7 第一次啟動時會寫出 `<store>-peers` 記錄；在那個時候，於更換密鑰途中重新啟動無法使用 `previous_secret_file`。

完整的清單見[第 15 章](15-upgrading.md)。

## Metrics

| Metric | 意義 |
| --- | --- |
| `emfor_peer_up{peer}` | 該成員存活時為 1。 |
| `emfor_peer_messages_rejected_total{reason}` | 依原因分類的丟棄訊息數。 |
| `emfor_peer_notifications_total{direction}` | 送出與收到的 `down` 通知。 |
| `emfor_peer_results_total{direction}` | 送出與收到的檢查結果。 |
| `emfor_peer_disagreements` | 依投票為 down、但在這裡是 up 的 backend 數。 |
| `emfor_peer_checks_skipped_total` | 只由投票決定的檢查時間次數。 |
| `emfor_peer_queue_dropped_total` | 因 peer task 落後而丟棄的訊息數。 |
