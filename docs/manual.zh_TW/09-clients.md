# 9. Client 函式庫

[手冊](README.md) · 上一章：[8. 如何選出 backend](08-choosing-a-backend.md) · 下一章：[10. DNS](10-dns.md)

client 函式庫只回答一個問題：「這個服務，現在我該連到哪個 `ip:port`？」它讀本機的 store，照[第 8 章](08-choosing-a-backend.md)的方式挑一個 backend，然後回傳。它不會往網路送任何東西，也不會重試、不會幫你連線、不做連線池：這些都還是你的應用程式要處理。

每個 client 都只有一個函式 `lookup(service, affinity_key)`，錯誤也都是同樣四種。下面每個範例都在 Debian 容器裡建置過，並且搭配一個正在執行的 emfor 0.7.0 daemon 跑過；函式庫的安裝方式請看[第 4 章](04-installing.md#client-函式庫)。

## 所有 client 的共通之處

### 答案

IPv4 是 `ip:port`，IPv6 是 `[ip]:port`，可以直接丟給連線函式。affinity key 只有 `balance = "sticky"` 才用得到；不給 key 和給空的 key 效果一樣。

### 錯誤

| 代碼 | 意義 | 訊息範例 |
| --- | --- | --- |
| `UNAVAILABLE` | store 不能用：不存在、讀不到、用的是另一個 LMDB build（client 來自別的發行版）、還沒初始化，或 `EMFOR_MAX_STALENESS` 的值無效。 | `store /var/lib/emfor/state.mdb does not exist; is the emfor daemon running?` |
| `INCOMPATIBLE` | store 的格式版本不是 1 也不是 2，或有記錄解析不了。 | |
| `NOT_FOUND` | store 裡沒有這個服務。 | `no service named "nope.svc.emfor"` |
| `ALL_DOWN` | 沒有 backend 可以選，而且 `on_all_down = "closed"`（或所有 backend 都被 drain）。 | `service db.svc.emfor: no backend is healthy, and on_all_down is closed` |

錯誤印出來的樣子是 `CODE: message`。各種錯誤的處理方式：

- `ALL_DOWN`：就 emfor 所知，這個服務已經 down 了。讓這個請求失敗，或改走備用方案。
- `NOT_FOUND`：通常是打錯字，或這台主機的設定裡沒有這個服務。
- `UNAVAILABLE`：daemon 沒在跑、你的使用者讀不到 store，或 client 來自別的發行版。這是主機的問題，不是服務的問題。

store 過期不算錯誤：結果由 `on_all_down` 決定（[第 8 章](08-choosing-a-backend.md#層級)）。

### 環境

行程第一次查詢時會讀這些變數，`fork` 之後子行程會再讀一次：

| 變數 | 預設值 | 意義 |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | store 的位置。`[store] path` 不是預設值時要設（FreeBSD port 用的是 `/var/db/emfor/state.mdb`）。 |
| `EMFOR_MAX_STALENESS` | `3s` | heartbeat 最多可以多舊：正整數加上 `ms` 或 `s`（`1500ms`、`5s`）。值無效的話，每次查詢都會失敗：`UNAVAILABLE: EMFOR_MAX_STALENESS="3" is invalid: use a positive whole number followed by ms or s, such as 3s or 1500ms`。 |
| `EMFOR_LIBRARY` | | 只有 Python 用：`libemfor` 不在標準目錄時，填它的路徑。 |

除此之外沒有別的設定。client 不會去讀 daemon 的設定檔。

### 權限

行程要能讀 store，還要能**寫 store 的 lock 檔**（`<store>-lock`），因為每個讀取者都要在 lock 檔裡登記。如果用預設的權限（0640 和 0660，擁有者是 daemon 的使用者和群組），就把應用程式的使用者加進 daemon 的群組：

```sh
# usermod -aG emfor www-data
```

不在這個群組裡的使用者會看到
`UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied`。
請看[第 12 章](12-operating.md#誰能讀取-store)。

### 執行緒、fork 與被替換的 store

- 多個執行緒可以同時呼叫 `lookup`。round-robin 計數器是每個行程各一份，從隨機的 backend 開始。
- `fork()` 之後的子行程也能用，就算父行程之前查過，或那一刻剛好有其他執行緒在查也沒關係：子行程會重新開啟 store。
- 查詢只在讀取的那段時間占用一個 LMDB reader slot。整台主機的 store 一共有 1024 個 slot。如果行程在查詢途中被砍掉，它的 slot 會一直被占著，等 daemon 來釋放，大約每分鐘一次。
- daemon 替換 store 檔案時（啟動時把損毀的 store 移到旁邊，或檔案被刪掉），執行中的應用程式大約 100 ms 內就會改用新檔案，不用重新啟動。
- store 檔案存在但開不起來時，最多每秒重試一次；store 不存在時，每次查詢都會重試，所以比 daemon 早啟動的應用程式，等 store 一出現就找得到。

不要只刪掉 `<store>-lock`。另外，如果有應用程式是以 root 或 daemon 的使用者身分執行，請先啟動 daemon，再啟動這些應用程式：lock 檔不存在時，第一個開啟 store 的行程會建立 lock 檔，擁有者是這個行程的使用者，之後用另一個使用者執行的 daemon 可能就啟動不了。

### 與 daemon 同一發行版

`libemfor` 裡面就是 daemon 自己用的那個 LMDB build，lock 檔的格式也取決於它：系統的 `liblmdb`、PHP 的 `dba`，或別的 emfor 發行版的 client，都沒辦法可靠地讀這個 store。請從和 daemon 相同的發行版安裝 client，升級時也一起升。版本不合時會回傳 `UNAVAILABLE`，訊息裡會寫出原因。

### 成本

數字取自 [docs/performance.md](../performance.md)（Apple M4 Max、Rust client；不是為了這份手冊量的）：

| | macOS | Linux（容器） |
| --- | --- | --- |
| 行程中的第一次查詢（開啟 store） | 443 µs | 151 µs |
| 查詢，單一執行緒，中位數 | 1.5 µs | 1.1 µs |
| 對 daemon 的 DNS A 查詢，中位數 | 24.8 µs | 4.6 µs |

Go、Python、Perl 和 PHP 還要再加上各自的呼叫開銷。執行緒加到大約四個之前，吞吐量會跟著上升；再多的話，速度就卡在 store 共用的 reader lock 上。一次 `sticky` 查詢，每個候選每一單位的權重都要算一次 MD5。結論是：每個請求都查一次，不要只在啟動時查。把答案快取起來，健康檢查就沒有意義了。

## C

```c
#include <emfor.h>
#include <stdio.h>

int main(int argc, char **argv)
{
    const char *service = argc > 1 ? argv[1] : "web.svc.emfor";
    char addr[EMFOR_ADDR_MAX];

    emfor_status st = emfor_lookup(service, NULL, addr, sizeof addr);
    if (st != EMFOR_OK) {
        fprintf(stderr, "%s: %s\n", emfor_status_name(st), emfor_last_error());
        return 1;
    }
    printf("%s\n", addr);
    return 0;
}
```

```console
$ cc app.c $(pkg-config --cflags --libs emfor) -o app
$ ./app
127.0.0.1:8080
$ ./app nope.svc.emfor
NOT_FOUND: no service named "nope.svc.emfor"
```

| 函式 | |
| --- | --- |
| `emfor_status emfor_lookup(const char *name, const char *affinity_key, char *out, uintptr_t out_len)` | 把位址和結尾的 NUL 寫進 `out`。`affinity_key` 可以是 `NULL`。失敗時不會動到 `out`。 |
| `const char *emfor_status_name(int status)` | `"OK"`、`"ALL_DOWN"` 等；回傳靜態字串。 |
| `const char *emfor_last_error(void)` | 這個執行緒上一次失敗呼叫的訊息，在這個執行緒下一次呼叫 emfor 之前都有效。 |

`emfor_status` 是 `EMFOR_OK`（0）、`EMFOR_UNAVAILABLE`（1）、
`EMFOR_INCOMPATIBLE`（2）、`EMFOR_NOT_FOUND`（3）、`EMFOR_ALL_DOWN`（4）或
`EMFOR_INVALID_ARGUMENT`（5：名稱或緩衝區是 `NULL`、字串不是 UTF-8，或緩衝區太小）。緩衝區用 `EMFOR_ADDR_MAX`（64）一定放得下。標頭檔另外還定義了 `EMFOR_MAJOR`、`EMFOR_MINOR` 和 `EMFOR_PATCH`。

如果 `libemfor` 不在 dynamic linker 會找的目錄裡，請加上 `-Wl,-rpath,<libdir>`，或執行 `ldconfig`。

## Go

```go
package main

import (
	"errors"
	"fmt"
	"os"

	emfor "github.com/embark-systems/emfor/clients/go"
)

func main() {
	addr, err := emfor.Lookup("web.svc.emfor", "") // "" 表示沒有 affinity key
	var e *emfor.Error
	switch {
	case err == nil:
		fmt.Println(addr)
	case errors.As(err, &e) && e.Code == emfor.AllDown:
		fmt.Fprintln(os.Stderr, "no healthy backend:", e.Message)
		os.Exit(1)
	default:
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
```

```sh
$ go get github.com/embark-systems/emfor/clients/go@v0.7.0
$ go run .
```

- `emfor.Lookup(name, affinityKey string) (string, error)`。
- 失敗時回傳 `*emfor.Error`，裡面有 `Code`（`emfor.Unavailable`、`emfor.Incompatible`、`emfor.NotFound`、`emfor.AllDown`）和 `Message`。如果名稱或 key 含有 NUL 位元組，或不是有效的 UTF-8，回傳的是一般的 error。
- 這個 package 用到 cgo 和 `pkg-config emfor`：需要 C 編譯器，不能用 `CGO_ENABLED=0` 建置。
- 模組的 tag 是 `clients/go/v0.7.0`。測試這個範例時，`go.mod` 用一行 `replace` 指向本機的 checkout。

## Python

```python
import sys
import emfor

try:
    addr = emfor.lookup("web.svc.emfor")
except emfor.AllDownError as e:
    sys.exit(f"no healthy backend: {e.message}")
except emfor.Error as e:
    sys.exit(f"{e.code}: {e.message}")
print(addr)
```

- `emfor.lookup(name, affinity_key=None)`。
- 錯誤都是 `emfor.Error` 的子類別：`UnavailableError`、`IncompatibleError`、`NotFoundError`、`AllDownError`，每個都有 `.code` 和 `.message`。
- 需要 Python 3.9 以上。這個 package 用 `ctypes`，不用編譯任何東西。
- 多個執行緒可以同時用（呼叫期間會釋放 GIL），`os.fork()` 之後也能用。
- 它從 `EMFOR_LIBRARY` 載入 `libemfor`；沒設這個變數時，就用名稱到標準目錄找。

## Perl

```perl
use strict;
use warnings;
use Emfor;

my $addr = eval { Emfor::lookup('web.svc.emfor') };
if (my $err = $@) {
    die $err unless ref $err;            # 無效的參數
    die "no healthy backend\n" if $err->code eq 'ALL_DOWN';
    die "$err\n";                        # "NOT_FOUND: ..."
}
print "$addr\n";
```

- `Emfor::lookup($name, $affinity_key)`；`undef` 或 `""` 表示沒有 key。
- 失敗時會 die 一個 `Emfor::Error`：有 `code`、`message`，當字串用時是 `"CODE: message"`。參數含有 NUL 位元組時，die 的是一般字串，開頭是 `Emfor::lookup: invalid argument`。
- 參數是字元字串，會以 UTF-8 傳下去。如果手上是位元組，要先解碼（`Encode::decode('UTF-8', $bytes)`），不然非 ASCII 的 affinity key 會選到和其他語言不一樣的 backend。
- 建置：在 `clients/perl` 裡執行 `perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`。

## PHP

```php
<?php
try {
    $addr = Emfor\lookup('web.svc.emfor');
} catch (Emfor\LookupException $e) {
    fwrite(STDERR, $e->getMessage() . "\n");   // "ALL_DOWN: ..."
    exit(1);
}
echo $addr, "\n";
```

你跑的每個 PHP minor 版本（PHP 8.1 以上）都要各建置一次擴充模組。請先把原始碼複製一份再建置，因為 `phpize` 會寫出自己的 Makefile：

```sh
$ mkdir -p /tmp/emfor-php
$ cp clients/php/config.m4 clients/php/emfor.c clients/php/emfor_arginfo.h /tmp/emfor-php/
$ cd /tmp/emfor-php && phpize && ./configure --enable-emfor && make
# make install
```

接著在 PHP 會載入的 ini 檔裡加上 `extension=emfor`（`php --ini` 會列出有哪些檔案）。

- `Emfor\lookup(string $name, ?string $affinity_key = null): string`。
- 失敗時丟出 `Emfor\LookupException`（是一種 `RuntimeException`）：`$e->errorCode` 是代碼名稱，`getCode()` 是 C 的狀態值，`getMessage()` 是 `"CODE: message"`。遇到 NUL 位元組或無效的 UTF-8，會丟出 `ValueError`。
- `pcntl_fork()` 之後也能用，也可以和 `dba` 擴充模組的 `lmdb` handler 同時使用。
- 用 PHP-FPM 時，每個 worker 行程都是各自獨立的 client。

## Rust

```text
# 你的應用程式的 Cargo.toml
[dependencies]
emfor-client = { path = "clients/rust" }   # 來自同一個 emfor 發行版
```

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    match emfor_client::lookup("web.svc.emfor", None) {
        Ok(addr) => {
            println!("{addr}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}"); // "ALL_DOWN: ..." 之類
            ExitCode::FAILURE
        }
    }
}
```

- `emfor_client::lookup(name: &str, affinity_key: Option<&str>) -> Result<String, Error>`。
- `Error` 有 `code`（`ErrorCode::Unavailable`、`Incompatible`、`NotFound`、`AllDown`）和訊息；用 `Display` 印出來是 `CODE: message`。
- 這個 crate 自己就內含 bundled 的 LMDB，所以不需要 `libemfor`。

## 不用 client 函式庫

- **DNS**：任何程式都可以透過 daemon 的 DNS server 解析 `web.svc.emfor`（[第 10 章](10-dns.md)）。A 和 AAAA 回答不帶 port。
- **命令列**：`emfor lookup web.svc.emfor` 會印出一個位址並以 0 結束；失敗時印出錯誤，並以 1（`ALL_DOWN`）、2（沒有這個服務）或 3（store 不能用）結束。適合寫 script；但每個請求都跑一次就太慢了。
