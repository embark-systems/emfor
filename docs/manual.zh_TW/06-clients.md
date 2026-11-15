# 6. Client 函式庫

[手冊](README.md) · 上一章：[5. 選擇 backend](05-backend-selection.md) · 下一章：[7. DNS](07-dns.md)

client 函式庫只有一個呼叫：查詢一個服務，拿回一個 backend 位址。它讀取本機的 store，所以只需要同一台主機上有執行中的 emfor daemon，其他什麼都不需要。

## 查詢

每個 client 都接受一個服務名稱與一個選用的 affinity key，回傳一個字串：

- IPv4 為 `ip:port`，例如 `10.0.0.11:8080`；
- IPv6 為 `[ip]:port`，例如 `[2001:db8::11]:8080`。

兩種形式都能直接交給大多數語言中「連線到 host:port」的函式。名稱比對不分大小寫，並忽略一個結尾的點。affinity key 只對 `balance = "sticky"` 有作用；空的 key 等於沒有 key。

每建立一條新連線就呼叫一次 `lookup`，不要只在啟動時呼叫一次：答案會隨 backend 故障與恢復而改變，而一次查詢只花大約一微秒。

## 錯誤

每個 client 失敗時都是四個代碼之一，各語言的名稱都相同：

| 代碼 | 意義 | 怎麼處理 |
| --- | --- | --- |
| `UNAVAILABLE` | store 無法使用：不存在、無法開啟、由其他 emfor 發行版的 LMDB 寫入、尚未初始化，或 `EMFOR_MAX_STALENESS` 無效。 | 確認 daemon 正在執行、`EMFOR_STORE`、檔案權限，以及 `libemfor` 與 daemon 來自同一發行版。訊息會說明是哪一項。 |
| `INCOMPATIBLE` | store 的格式版本不同，或某筆記錄無法讀取。 | 安裝與 daemon 同一發行版的 client。 |
| `NOT_FOUND` | 沒有這個名稱的服務。 | 對照設定檔檢查名稱。 |
| `ALL_DOWN` | 沒有健康的 backend，且服務設定 `on_all_down = "closed"`。 | 讓請求失敗，或稍後重試。 |

store 過期本身不是錯誤：它讓每個 backend 都算作不健康，所以 `open` 的服務仍會回傳位址，`closed` 的服務則以 `ALL_DOWN` 失敗。

## 設定

client 沒有設定用的呼叫。行程中第一次查詢時會讀取兩個環境變數（`fork` 之後的子行程會再讀一次）：

| 變數 | 預設值 | 意義 |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | store 的路徑；必須與 daemon 的 `[store] path` 相同。 |
| `EMFOR_MAX_STALENESS` | `3s` | heartbeat 可以多舊：正整數加上 `ms` 或 `s`，例如 `1500ms`。值無效時每次查詢都以 `UNAVAILABLE` 失敗。 |

## 範例

**Rust**

```rust
match emfor_client::lookup("api.svc.emfor", Some("user-42")) {
    Ok(addr) => println!("connect to {addr}"),
    Err(e) => eprintln!("{e}"), // "ALL_DOWN: ..."; e.code is an ErrorCode
}
```

**C**

```c
#include <emfor.h>

char addr[EMFOR_ADDR_MAX];
emfor_status st = emfor_lookup("api.svc.emfor", "user-42", addr, sizeof addr);
if (st == EMFOR_OK)
    printf("connect to %s\n", addr);
else
    fprintf(stderr, "%s: %s\n", emfor_status_name(st), emfor_last_error());
```

以 `cc app.c $(pkg-config --cflags --libs emfor)` 建置。C 另外有 `EMFOR_INVALID_ARGUMENT`，用於參數為 NULL、不是 UTF-8，或緩衝區太小；`EMFOR_ADDR_MAX` 位元組一定放得下。

**Go**

```go
import "github.com/embark-systems/emfor/clients/go"

addr, err := emfor.Lookup("api.svc.emfor", "user-42")
var e *emfor.Error
if errors.As(err, &e) && e.Code == emfor.AllDown {
	// no healthy backend
}
```

**Python**

```python
import emfor

try:
    addr = emfor.lookup("api.svc.emfor", affinity_key="user-42")
except emfor.AllDownError:
    ...
except emfor.Error as e:
    log.warning("%s: %s", e.code, e.message)
```

**Perl**

```perl
use Emfor;

my $addr = eval { Emfor::lookup('api.svc.emfor', 'user-42') };
warn "$@\n" if $@;   # an Emfor::Error with ->code and ->message
```

**PHP**

```php
try {
    $addr = Emfor\lookup('api.svc.emfor', 'user-42');
} catch (Emfor\LookupException $e) {
    error_log($e->getMessage());   // $e->errorCode is "ALL_DOWN" etc.
}
```

各 client 的 README 說明如何安裝以及完整的 API（英文）：[Rust](../../clients/rust/README.md)、[C](../../clients/c/README.md)、[Go](../../clients/go/README.md)、[Python](../../clients/python/README.md)、[Perl](../../clients/perl/README.md)、[PHP](../../clients/php/README.md)。

## 從 shell 查詢：emfor lookup

`emfor lookup` 讓沒有 client 函式庫的 script 與 cron 工作，在 shell 裡得到 client 函式庫會給的答案，加上 `--explain` 還能知道為什麼。

```console
$ emfor lookup api.svc.emfor
10.0.0.11:8080
$ emfor lookup api.svc.emfor --key user-42
```

它讀設定檔（`--config`，與 `emfor status` 相同）`[store] path` 的 store，並像 client 一樣套用 `EMFOR_MAX_STALENESS`，從不寫入 store。`--key` 是 `sticky` 服務的黏著鍵。

每次執行都是新的行程，所以輪流類的 `balance`（`round-robin`、`weighted`、`locality`、`fastest`，以及沒有 key 的 `sticky`）會從隨機的起點給出其中一個候選：兩次執行可能印出不同的 backend。需要每次都得到同一個答案的 script，請用 `failover` 服務，或 `sticky` 加 `--key`。

`--explain` 說明原因：store 是否 stale、候選來自哪一層（[第 5 章](05-backend-selection.md)的層級）、每個被排除的 backend 與原因，以及 `balance` 怎麼挑。

```console
$ emfor lookup api.svc.emfor --explain
service: api.svc.emfor
answering: as a client library answers
store: /var/lib/emfor/state.mdb (heartbeat 0.4s ago, fresh)
balance: round-robin  on_all_down: open
tier: healthy primary backends whose last check passed
candidates:
  10.0.0.11:8080
excluded:
  10.0.0.12:8080  last check failed
  10.0.0.13:8080  backup held back
  10.0.0.14:8080  unhealthy
selection: round-robin: the candidates in turn
  the actual choice depends on the rotation start of the process, which is random in a new process
answer: 10.0.0.11:8080
```

原因有 `stale`（stale 的 store 裡沒有 backend 算 healthy 或 `unknown`）、`unhealthy`、`unknown`（有更好的層級有候選）、`last check failed`、`backup held back`、`not in the pool`（`fastest`）與 `not at this site`（`locality`）。慢啟動中的候選會顯示步數與有效權重。`failover` 與有 key 的 `sticky` 會指出它選的 backend；其他方法因為答案取決於行程的輪流位置，只列出候選與規則。`--explain` 不會推進任何輪流位置。

`--json` 輸出一個 JSON 物件（`service`、`via`、`family`、`store`、`balance`、`on_all_down`、`result`，加上 `--explain` 時還有 `explain`）；之後的版本只會增加欄位。文字輸出不是穩定的介面。

`--dns A` 與 `--dns AAAA`（兩者都隱含 `--explain`）說明 daemon 的 DNS server 對 A 或 AAAA 查詢會怎麼回答：只看該 family 的 backend、store 一律視為 fresh、`sticky` 沒有 key、`weighted` 用 daemon 的 smooth weighted round-robin。它讀的是 store，而 store 在 daemon 決定新的 `reason` 之後最多晚約一秒才寫入，所以在那一秒內可能與真正的 DNS 答案不同；daemon 的輪流計數器在它的記憶體裡，不會顯示。SRV 的回答不做 explain。

| 結束代碼 | 意義 |
| --- | --- |
| 0 | 印出了位址（用 `--dns` 時：服務在該 family 有候選） |
| 1 | 沒有 backend 可選：`ALL_DOWN`，或用 `--dns` 時沒有該 family 的 backend |
| 2 | 沒有這個服務、命令列無效，或讀不到設定檔 |
| 3 | store 無法使用（`UNAVAILABLE`），或格式這個指令讀不了（`INCOMPATIBLE`） |

`--explain` 不會改變結束代碼，查詢失敗時仍會印出已知的內容。

## 執行緒、行程與 fork

- 可以從多個執行緒同時查詢。
- 每個行程只開啟 store 一次並重複使用。一次查詢只在讀取期間占用一個 LMDB reader slot。store 共有 1024 個 slot，由主機上所有行程共用；查詢途中被殺掉的行程會留下一個被占用的 slot，直到 daemon 釋放它（大約每分鐘一次）。
- `fork` 之後，子行程在第一次查詢時重新開啟 store；即使父行程在 fork 前查詢過（php-fpm、prefork 型伺服器），或 fork 的當下父行程有其他執行緒正在查詢，也沒問題。
- 在 FreeBSD 上，`fork` 會等到行程內沒有查詢還在 store 裡，並擋住新的查詢，直到 fork 在父行程與子行程都回傳；否則在別的執行緒查詢的當下 fork，子行程的第一次查詢可能永遠卡住，因為 FreeBSD 的執行緒函式庫會把它自己的一把鎖在子行程裡留成已鎖住的狀態。等待的時間只有一次讀取那麼長；fork 進行期間，查詢也同樣會等。有執行緒在查詢途中被 `SIGSTOP`，fork 會等到它繼續。FreeBSD 上的 `libemfor` 不能被卸載（`dlclose` 之後它仍然留在記憶體裡），因為它的 fork handler 要到行程結束才會解除註冊；把 `libemfor.a` 靜態連進會被卸載的模組，或在自己的 cdylib 裡依賴 Rust crate，必須自行加 `-Wl,-z,nodelete`。
- FreeBSD 上這個 guard 沒有涵蓋的情形：在 signal handler 裡 fork，而被中斷的正是同一個執行緒正在進行的查詢（fork 會回傳，但子行程的第一次查詢仍可能卡住，和沒有 guard 時一樣）；以及同一個行程裡其他使用 process-shared mutex 的程式（例如另一個 LMDB environment），它們仍可能讓子行程在自己的查詢裡卡住。不要在 signal handler 裡 fork；其餘的情形函式庫沒有辦法處理。
- round-robin 計數器是每個行程各自一份。

## store 檔案被替換或變大時

如果 store 檔案在應用程式執行期間被替換（daemon 把損毀的 store 移到一旁，或有人刪掉它而 daemon 建立了新的），client 會在大約 100 ms 內察覺並切換到新檔案；不需要重新啟動應用程式。daemon 以較大的 `[store] map_size` 重新啟動、store 長到超過原本的大小時也一樣。

store 檔案存在但開不起來時（LMDB 格式不同、沒有權限、不是 store），client 最多每秒重試一次，期間的查詢都以同一個 `UNAVAILABLE` 錯誤失敗。store 檔案不存在時，每次查詢都會重試，所以比 daemon 先啟動的應用程式在 store 出現後就能讀到。

不要單獨刪除 lock 檔（`<store>-lock`）。請在以 root 或 daemon 使用者身分執行的應用程式之前啟動 daemon：lock 檔不存在時，第一個開啟 store 的行程會建立它，擁有者是該行程的使用者，以另一個使用者執行的 daemon 之後可能無法啟動。

## 與 daemon 同一發行版

client 函式庫內含 daemon 自己的 LMDB build，而 store 的 lock 檔格式取決於它。`libemfor`（以及各語言包裝）要與 daemon 來自同一個 emfor 發行版，並一起升級。版本不符會以 `UNAVAILABLE` 呈現，訊息會說明原因。

### store 格式版本

daemon 寫入 store 格式版本 2，它新增了 backend 欄位 `backup`。這個發行版的 client 能讀版本 1 與 2，所以搭配較舊的 daemon 也能用；較舊的 client 讀到版本 2 的 store 時，每次查詢都會以 `INCOMPATIBLE` 失敗。升級時，請先升級主機上的 client（以及 `emfor status`），再升級 daemon。

## 效能

單一執行緒一次查詢約 1–1.5 µs。吞吐量隨執行緒增加到大約四個為止，之後由 store 共用的 reader lock 決定速度（在 Linux 上合計約每秒兩百萬次查詢）。量測數據見 [docs/performance.md](../performance.md)。
