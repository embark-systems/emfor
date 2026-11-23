# 11. 聯防群組

[手冊](README.md) · 上一章：[10. DNS](10-dns.md) · 下一章：[12. 維運 emfor](12-operating.md)

沒有聯防群組時，每個 daemon 都自己做決定。這樣很單純，遇到什麼網路故障都撐得住，但有兩個代價：每台主機都要等到自己的檢查失敗 `fall` 次才會發現故障，而且每台主機都要檢查每一個 backend。把同一個資料中心的 daemon 組成**聯防群組**（peer group），就能分攤這些工作：

- **故障通知。** 某個成員看到 backend 從 `healthy` 變成 `unhealthy` 時，會通知其他成員。收到通知的成員，如果那個 backend 在它那邊還是 `healthy`，就開始一輪**快速重查**：每 200 ms 檢查一次（如果 `interval` 更短，就每個 `interval` 一次），直到有一次檢查通過、backend 變成 `unhealthy`，或已經檢查了 `fall` 次為止。所以整個群組大約 `fall × 200 ms` 內就會跟上第一個成員。
- **投票。** 存活的成員有三台以上時，每個 backend 都會有三台成員當它的**負責主機**（assigned checker）。三台裡有兩台看到它 down，它在每個成員上都算 down，不管那個成員自己的檢查結果是什麼。
- **少做一些檢查。** 如果一個成員不是某個 backend 的負責主機，它可以直接採用投票結果，不必自己檢查。

查詢和 DNS 還是只讀本機的 daemon。如果某個成員連不到其他成員，就跟沒有群組時一樣，照自己的檢查來決定。

## 設定

每個成員都要有一個 `[peers]` table 和一個 `site`：

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
| `listen` | 必填 | 這台主機的 `ip:port`（UDP），不能是 `0.0.0.0` 或 `::`。改這個值，或新增、移除 `[peers]`，都要重新啟動。 |
| `members` | 必填 | 2–64 個不重複的位址，全部要和 `listen` 同一個位址族，而且要包含 `listen`。可以 reload。 |
| `secret_file` | 必填 | 共用的密鑰（[見下文](#密鑰)）。可以 reload，daemon 也會監看檔案有沒有變動。 |
| `previous_secret_file` | 無 | 只在啟動時讀取，給更換密鑰途中重新啟動的情況用（[見下文](#更換密鑰)）。 |
| `encrypt` | `true` | 加密訊息。設成 `false` 只簽署、不加密。可以 reload。 |
| `accept_plaintext` | `encrypt` 是 `true` 時為 `false`，否則為 `true` | 接受有簽署但沒加密的訊息。設成 `false` 又同時設 `encrypt = false`，會被當成錯誤。可以 reload。 |
| `cipher` | `"chacha20-poly1305"` | 加密訊息用的演算法：`"chacha20-poly1305"`（`EMP2`／`EMP3`）或 `"aes-256-gcm"`（`EMP4`）。`"aes-256-gcm"` 要搭配 `encrypt = true`。可以 reload。見[下面](#選用-aes-gcm)。 |

有 `[peers]` 時一定要設 `site`。群組成員應該放在同一個資料中心：投票的前提是，一個成員看到的狀況，其他成員也會看到。

所有成員都應該列出相同的服務，檢查定義也要一樣。成員之間會比對每個服務檢查的**摘要**（digest）（檢查名稱，或 plugin 加上它的 command 和 `env_pass`，以及 `check_options` 和 `timeout`）。摘要不同的服務，成員會忽略它的票和通知，`emfor status` 也會顯示摘要不符。`interval`、`rise`、`fall`、`balance`、`on_all_down` 和 backend 清單則可以不同，但服務或 backend 清單不同會少掉票數（見[下面](#成員不一樣時)）。

成員之間要開放這個 UDP port。每則訊息是一個最多 1200 位元組的 datagram；每個成員每秒會送一次 heartbeat 給其他每個成員。

### 讓某個服務不參加

在服務上（或在 `[defaults]` 裡）設 `peers = false`，這個服務在這台主機上就不參加群組：它在這裡每個 interval 都會檢查，結果不會送出去，別人送來的票和通知也都忽略。這台主機會在 heartbeat 裡告訴其他成員，所以其他成員不會把它算成這個服務的負責主機。

### 成員不一樣時

下面的排名只看服務名稱、backend 位址和存活的成員（扣掉讓這個服務不參加的成員），不知道哪個成員有這個服務或 backend。所以成員列的服務和 backend 不一樣時：

- 沒有這個服務或 backend 的成員，在其他成員的排名裡仍然是候選，但它不會檢查這個 backend，也不會送結果。其他成員拿到的票就少於三張。剩下兩張時，要兩張都是 `down`，投票才會套用 `down`；只有一個 peer 說 `down` 不夠。這個成員對自己沒有的服務或 backend，收到的訊息直接忽略（只有一行 `debug`）；其他成員會把缺少的票看成「沉默的負責主機」（見[下面](#沉默的負責主機)）。
- 摘要不同的成員會送結果，但其他成員只存起來、不算票，所以它的名額也同樣浪費掉。`emfor status` 只看得到這一種差異：摘要不符，顯示到那個成員最後一則訊息之後 60 秒。
- `interval`、`rise`、`fall`、`balance` 和 `on_all_down` 不同，不影響投票。

#### 沉默的負責主機

某個成員是某個 backend 的負責主機、存活、也沒有讓這個服務不參加，卻沒有送這個 backend 的新鮮結果，就是**沉默的負責主機**。每個成員每秒用已經保留的資料找一次，不需要任何新訊息。沉默超過「10 秒」與「服務 `interval` 的 6 倍」中較長的那一個，才會回報，所以剛啟動、剛變存活的成員，以及掉一個封包，都不會回報。回報出現在四個地方：

- `emfor status`：`peers: no results from <member> for <service> (<n> backends)`，在摘要不符的行之後；
- `emfor status --json`：`peers.silent_checkers`，每個 backend 一筆 `{"service", "backend", "member"}`；沒有沉默的成員時沒有這個 key；
- metric `emfor_peer_silent_checkers`，就是這些筆數；
- log：某個成員的某個服務第一次有 backend 沉默時一行 `warn`，都恢復時一行 `info`。

原因不會告訴你。成員沒有這個服務或 backend、只有這台收不到它的封包、它的檢查一直回 `error`（`error` 不送結果），看起來都一樣。檢查定義不同的成員顯示成摘要不符；讓服務不參加的成員不是負責主機。沉默的負責主機不會改變投票：它仍然佔三個名額中的一個。

請讓每個成員的服務和 backend 清單一樣，例如用同一份 `conf.d` 檔產生。只有部分成員有的服務，在有的那幾台設 `peers = false`：它們自己檢查，其他成員也不會把它們算進去。要讓某個成員用不同的 backend 清單，請給它另一個服務名稱。

### 好幾個機房

聯防群組是給一個機房用的。有兩個以上的機房時，**每個機房各自一個群組**，不要跨機房：

- 每個機房有自己的 `site`、自己的 `members` 清單、自己的密鑰。`site` 和收訊者不同的成員會被拒絕（`reason="site"`，見[第 16 章](16-troubleshooting.md)），用另一把密鑰簽的訊息則會先在驗證那一步失敗。同一個網路上的兩個群組不可以共用密鑰。
- 不要把兩個機房的成員放進同一個群組。投票假設「一個成員看到的，其他成員也會看到」，跨機房時這不成立：和另一個機房的 backend 之間斷線的成員，會對其他人連得到的 backend 投 `down`。每則訊息都是單一個 UDP datagram，沒有重送；成員在最近 3 秒內有收到有效訊息才算存活，時間和收訊者的時鐘差超過 5 秒的訊息會被丟掉。誰存活一變動，就會讓「哪些成員負責檢查哪個 backend」重新洗牌。`tools/peer-wan-test.sh` 對三個成員的聯防訊息各加了 60 秒的延遲或丟包：延遲 100 ms、延遲 1.5 秒 ± 0.5 秒、丟包 10 %、丟包 30 %，A 全程都看到另外兩個成員存活；丟包 50 % 時，60 秒裡有 6 秒少於兩個存活。從「兩張 `down` 票讓 A 顯示 backend 為 `unhealthy`」所需的時間來看，沒有干擾時是 1 秒，有干擾時是 2 到 5 秒。所以一般的跨機房連線不會弄壞群組；掉一半封包、或延遲超過 3 秒的連線才會。
- 另一個機房的 backend 仍然是一個普通的 backend。每個群組從自己這一側檢查它，所以它的狀態就是這個機房看到的：兩個機房之間的連線斷了，雙方可能都把對方的 backend 判成 `unhealthy`，各自照自己看到的運作。在 backend 上設 `site`，並用 `balance = "locality"`（[第 8 章](08-choosing-a-backend.md)），讓主機優先用自己機房的 backend，沒有任何一個通過時才用別的機房的；也可以把它們標成 `backup = true`。
- 跨廣域網路的檢查，成本由執行它的主機承擔。在群組裡，近的機房有 3 個被指派的成員檢查每個遠端 backend，其他成員採用它們的結果；沒有群組（`peers = false`）時，每台主機都要自己檢查。這種服務請保留 `peers` 的預設值。

這些數字來自 docker 裡的三個 daemon，在聯防的 port 上用 `tc netem` 加干擾；時鐘差異、真實的連線，以及遠端 backend 的檢查，都沒有試過。

## 投票如何運作

對每個 backend，每個成員都會把候選（自己，以及其他存活、而且沒有讓這個服務不參加的成員）用 `<service>/<backend>/<member listen>` 的 MD5 雜湊排名，取前三名當負責主機。每個成員算出來的都是同樣三台，不需要互相溝通。候選不到三台時就沒有負責主機，每個成員各自檢查。

每次檢查完，成員會統計這三台最新的結果（結果必須夠新，在 `3 × interval` 之內、來自存活的成員，而且摘要相同，才算數）：

| 票 | 套用的結果 |
| --- | --- |
| 沒有負責主機，或沒有票 | 這個成員自己的結果 |
| 2 票以上 `down` | `down`，reason 是 `peers <d>/<f>: <reason of the first down vote>` |
| 其他情況 | 這個成員自己的結果；如果是 `down`，reason 是 `local: <reason>` |

所以就算一個成員自己的檢查通過，只要三台負責主機裡有兩台看到 backend down，它還是可能把 backend 標成 down。這就叫**意見不一致**（disagreement），會以 `info` 記一次 log：
`backend <backend> of <service> is down at <d>/<f> peers but up here`，並計入 `emfor_peer_disagreements`。意見不一致通常表示這台主機和 backend 之間的網路有問題，或是檢查定義不一樣。

不是某個 backend 負責主機的成員，在兩張夠新的票意見一致時，會跳過自己的檢查（次數記在 `emfor_peer_checks_skipped_total`）。不過遇到下面幾種情況，它還是會自己檢查：已經連續跳過兩輪、自己最近一次的結果不是 `up`、正在快速重查，或票不是新的。每個結果只會在一個檢查時間點計入，所以其他成員的結果絕對不會讓 `rise` 或 `fall` 前進兩次。

`emfor status` 會顯示有幾個成員存活，但不會列出負責主機是哪幾台；要看的話，`emfor status --json` 會為每個 backend 列出來（`assigned_checkers`，[第 12 章](12-operating.md#emfor-status)）。文字輸出只顯示這台主機自己的情況：如果它不是畫面上某些 backend 的負責主機，`peers:` 那一行結尾會是 `, checks <n>/<m> backends here`，那些 backend 會標上 `voted`；如果畫面上的 backend 都沒有負責主機，那一行結尾會是 `, too few live members to vote`。三個成員的群組裡，每個成員都是每個 backend 的負責主機，所以三個都在跑時，那一行只有：

```text
peers: 2/2 live
```

三個成員停掉一個之後：

```text
peers: 1/2 live, not live 127.0.0.3:5302, too few live members to vote
```

（這裡算的是**其他**成員。）只要最近 3 秒內收到某個成員送來的有效訊息，那個成員就算存活。log 會先寫
`peer group on 127.0.0.1:5302 with 2 other members`，接著是
`peer 127.0.0.2:5302 is live`，後來是
`peer 127.0.0.3:5302 is not live (no valid message for 3s)`。

這些輸出來自同一個 Debian 容器上的三個 daemon（`127.0.0.1`–`127.0.0.3`）；跨真實主機、走真實網路的群組，沒有為了這份手冊試過。

## 密鑰

所有成員共用一把密鑰。訊息用它驗證；設了 `encrypt = true` 時，也用它加密。建立一次，再複製到每個成員：

```sh
# head -c 32 /dev/urandom | base64 > /etc/emfor/peers.key
# chown root:emfor /etc/emfor/peers.key
# chmod 0640 /etc/emfor/peers.key
```

這個檔案必須：

- 是絕對路徑，而且 daemon 讀得到；
- 擁有者是 root 或 daemon 的使用者（群組沒關係）；
- 其他人不能讀，群組和其他人都不能寫；
- 去掉結尾的一個換行後至少有 32 位元組，整個檔案最多 65536 位元組。

檔案裡的位元組就是密鑰，不一定要是 base64。`emfor check-config` 會拿檔案擁有者和執行它的使用者比對：像上面那樣建立的 `root:emfor` 檔案，用 daemon 的使用者或 root 執行都會通過；但如果檔案擁有者是 `emfor`，用 root 執行檢查就不會通過。

### 訊息格式

| 格式 | 何時使用 | 保護 |
| --- | --- | --- |
| `EMP1` | `encrypt = false` | HMAC-SHA256：有驗證，但在網路上看得到內容。 |
| `EMP2` | `encrypt = true`，送給還不確定能讀 `EMP3` 的成員（0.6 和更早的版本） | ChaCha20-Poly1305 |
| `EMP3` | `encrypt = true`，送給送過 `EMP3` 或宣告過支援它的成員 | XChaCha20-Poly1305 |
| `EMP4` | `encrypt = true` 且 `cipher = "aes-256-gcm"`，送給每個成員 | AES-256-GCM |

成員一開始對所有人都送 `EMP2`；一旦從某個成員的訊息看出它能讀 `EMP3`，就改對那個成員送 `EMP3`，log 會寫
`sending EMP3 to peer 127.0.0.2:5302 from now on`。這部分你什麼都不用設定。

### 選用 AES-GCM

設了 `cipher = "aes-256-gcm"`（並且 `encrypt = true`），成員就對每個成員送 `EMP4`，不再在 `EMP2` 和 `EMP3` 之間挑。規定要用 AES-GCM、不能用 ChaCha20 時才需要它；除此之外不會比較安全，預設也沒變。每個 daemon 都一律讀得懂 `EMP4`，不管自己的 `cipher` 和 `accept_plaintext` 怎麼設。

`EMP4` 訊息帶一個計數器：daemon 每次啟動從 1 開始，每送一份 datagram 加 1。計數器就是 nonce，所以同一把金鑰下 nonce 不會重複，而 daemon 每次啟動也都用自己的金鑰。

**升級順序。** 舊版成員不認得 `EMP4`：它會丟掉訊息並計為 `malformed`，在送訊息的那個成員眼裡它就變成「不 live」。所以請先把每個成員都升級，確定全部都讀得懂 `EMP4` 之後才設 `cipher`；每個成員改完設定後 reload（`SIGHUP`）就生效。要退回去，就拿掉 `cipher` 再 reload：下一份訊息又是 `EMP2`，還在送 `EMP4` 的成員照樣讀得懂。

每則訊息都帶有時間戳記、boot id 和序號。下面這些訊息都會被丟掉：重送的、太舊的、格式錯誤的、簽章不對的、來自不在 `members` 裡的位址的、來自別的 site 的，以及（預設情況下）沒加密的。丟掉的訊息會分別以 `replay`、`stale`、`malformed`、`auth`、`sender`、`site` 或 `plaintext` 計入 `emfor_peer_messages_rejected_total{reason}`。

「太舊」是拿接收端自己的時鐘來比，時間戳記在未來也一樣算：訊息的時間戳記和接收端的時鐘差超過 **5 秒**，不論往前或往後，都會以 `stale` 丟掉。這個上限是固定的。所以各成員的時鐘要校準到遠小於 5 秒（chrony 或 ntpd 做得到）；時鐘差得比這還多的成員，其他成員完全收不到它的訊息，在他們看來它就是不在線上。

## 更換密鑰

daemon 會自己發現 `secret_file` 有變動（Linux 上用 inotify，FreeBSD 和 macOS 上用 kqueue，另外不管怎樣每 30 秒都會讀一次），不用 reload。發現變動後，daemon 會：

- 繼續用舊的密鑰**簽署** 60 秒；
- 繼續**接受**舊的密鑰 120 秒。

所以只要**在 60 秒內**把新的密鑰寫到每個成員上，就不會掉任何訊息。

```sh
# head -c 32 /dev/urandom | base64 > /tmp/peers.key.new
# for h in host1 host2 host3; do
>   scp /tmp/peers.key.new $h:/etc/emfor/peers.key.new &&
>   ssh $h 'chown root:emfor /etc/emfor/peers.key.new && chmod 0640 /etc/emfor/peers.key.new &&
>           mv /etc/emfor/peers.key.new /etc/emfor/peers.key'
> done
```

用 `mv`（rename）來替換檔案，daemon 就不會讀到只寫了一半的檔案。讀不到或無效的檔案會被忽略：daemon 會繼續用最後一把有效的密鑰，並且最多每分鐘警告一次。請留意
`emfor_peer_messages_rejected_total{reason="auth"}`：成員用的密鑰不一樣時，這個值會上升。

### 更換途中重新啟動

如果 daemon 在更換途中重新啟動，它只知道新的密鑰，所以其他成員還在用舊密鑰簽署時，送來的訊息都會被丟掉，直到它們也換過去為止。要撐過這段時間，就把舊的密鑰留在 `previous_secret_file`：

```toml
site = "dc1"

[peers]
listen = "10.0.1.11:5302"
members = ["10.0.1.11:5302", "10.0.1.12:5302", "10.0.1.13:5302"]
secret_file = "/etc/emfor/peers.key"
previous_secret_file = "/etc/emfor/peers.key.old"
```

`previous_secret_file` 只有在下列條件全部成立時才會用到：daemon 的記錄
`<store>-peers`（`/var/lib/emfor/state.mdb-peers`，權限 0600）顯示被換掉的正是那把密鑰，而且是在 120 秒以內換的。這份記錄存的是 SHA-256 指紋和時間，從來不存密鑰本身。所以舊的密鑰不會因為某個檔案還留著它，之後又被拿回來用。**從 0.6 升級後第一次啟動**時還沒有這份記錄，`previous_secret_file` 會被忽略，並記一行 `info`。

### 成員很多的時候

上面的迴圈一台接一台改，每次 `scp` 和 `ssh` 都要時間；成員有十幾台以上，就可能超過 60 秒。做法是先備好、再一起切換：先把新密鑰複製到每個成員的 `/etc/emfor/peers.key.new`（沒有東西會讀這個名字），逐台確認擁有者和權限，然後才用平行的工具（pdsh、設了 `forks` 的 Ansible、`xargs -P`）在所有成員上同一時間執行 `mv`。rename 不花時間，所以差距只有幾秒。之後看 `emfor_peer_messages_rejected_total{reason="auth"}`：沒切換的成員，會在其他所有成員上出現。

如果有成員錯過了時間窗，它還是用舊密鑰簽名，其他成員會丟掉它的訊息，直到你把它的檔案也換掉；其他成員不用修什麼。

## 從雲端 API 取得成員

`members` 可以靠 reload 修改，所以主機會來來去去的群組，可以用 timer 讓成員清單保持最新。
[examples/deploy/emfor-peers-sync.sh](../../examples/deploy/emfor-peers-sync.sh)
會從標準輸入讀成員清單，寫進一個只有 `[peers] members` 的 `conf.d` 檔案，再用 daemon 的使用者執行 `emfor check-config`；檢查失敗就把舊檔案放回去，最後 reload：

```sh
# some-command-that-lists-the-hosts | /usr/local/sbin/emfor-peers-sync.sh /etc/emfor/conf.d/50-peers.toml
```

如果清單少於 2 個或多於 64 個項目（通常是 API 回了空的結果），或有一行不是位址，腳本會保留舊的檔案。`[peers]` 的其他設定留在主設定檔裡。透過 reload 加入的成員，一開始算不存活；被移除的成員則會被遺忘。根據 CHANGELOG，這支腳本在測試容器裡跑過；但沒有為了這份手冊跑過。

## 從 0.6 升級群組

- 0.7 預設會拒絕沒加密的（`EMP1`）訊息。如果你的 0.6 群組是用 `encrypt = false` 在跑，請在所有成員上開啟加密；或是在所有成員都升級完之前，先在 0.7 的成員上設 `accept_plaintext = true`。
- 密鑰檔現在的擁有者必須是 root 或 daemon 的使用者。
- 0.7 第一次啟動時才會寫出 `<store>-peers` 記錄；所以如果剛好在這個時間點、更換密鑰途中重新啟動，就用不到 `previous_secret_file`。

完整的清單請看[第 15 章](15-upgrading.md)。

## Metrics

| Metric | 意義 |
| --- | --- |
| `emfor_peer_up{peer}` | 該成員存活時為 1。 |
| `emfor_peer_messages_rejected_total{reason}` | 被丟掉的訊息數，依原因分類。 |
| `emfor_peer_notifications_total{direction}` | 送出和收到的 `down` 通知。 |
| `emfor_peer_results_total{direction}` | 送出和收到的檢查結果。 |
| `emfor_peer_disagreements` | 投票結果是 down、但在這裡是 up 的 backend 數。 |
| `emfor_peer_silent_checkers` | 沒有送結果的負責主機數（以服務、backend、成員各算一筆）。 |
| `emfor_peer_checks_skipped_total` | 只靠投票決定的檢查時間點次數。 |
| `emfor_peer_queue_dropped_total` | peer task 跟不上而丟掉的訊息數。 |
