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

## 執行緒、行程與 fork

- 可以從多個執行緒同時查詢。
- 每個行程只開啟 store 一次並重複使用。一次查詢只在讀取期間占用一個 LMDB reader slot。store 共有 1024 個 slot，由主機上所有行程共用；查詢途中被殺掉的行程會留下一個被占用的 slot，直到 daemon 釋放它（大約每分鐘一次）。
- `fork` 之後，子行程在第一次查詢時重新開啟 store；即使父行程在 fork 前查詢過也沒問題（php-fpm、prefork 型伺服器）。
- round-robin 計數器是每個行程各自一份。

## store 檔案被替換時

如果 store 檔案在應用程式執行期間被替換（daemon 把損毀的 store 移到一旁，或有人刪掉它而 daemon 建立了新的），client 會在大約 100 ms 內察覺並切換到新檔案；不需要重新啟動應用程式。

不要單獨刪除 lock 檔（`<store>-lock`）。請在以 root 或 daemon 使用者身分執行的應用程式之前啟動 daemon：lock 檔不存在時，第一個開啟 store 的行程會建立它，擁有者是該行程的使用者，以另一個使用者執行的 daemon 之後可能無法啟動。

## 與 daemon 同一發行版

client 函式庫內含 daemon 自己的 LMDB build，而 store 的 lock 檔格式取決於它。`libemfor`（以及各語言包裝）要與 daemon 來自同一個 emfor 發行版，並一起升級。版本不符會以 `UNAVAILABLE` 呈現，訊息會說明原因。

## 效能

單一執行緒一次查詢約 1–1.5 µs。吞吐量隨執行緒增加到大約四個為止，之後由 store 共用的 reader lock 決定速度（在 Linux 上合計約每秒兩百萬次查詢）。量測數據見 [docs/performance.md](../performance.md)。
