# 9. Client 函式庫

[手冊](README.md) · 上一章：[8. 如何選出 backend](08-choosing-a-backend.md) · 下一章：[10. DNS](10-dns.md)

client 函式庫只回答一個問題：「這個服務，現在我該連到哪個 `ip:port`？」它讀取本機的 store，依[第 8 章](08-choosing-a-backend.md)所述挑出一個 backend，然後回傳。它不經網路送出任何東西，也不重試、不連線、不做連線池：這些都留在你的應用程式裡。

每個 client 都只有一個函式 `lookup(service, affinity_key)`，以及同樣的四種錯誤。下面每個範例都在 Debian 容器中、對執行中的 emfor 0.7.0 daemon 建置並執行過；請依[第 4 章](04-installing.md#client-函式庫)安裝函式庫。

## 所有 client 的共通之處

### 答案

IPv4 為 `ip:port`，IPv6 為 `[ip]:port`，可以直接交給連線函式。affinity key 只對 `balance = "sticky"` 有作用；沒有 key 與空的 key 相同。

### 錯誤

| 代碼 | 意義 | 訊息範例 |
| --- | --- | --- |
| `UNAVAILABLE` | store 無法使用：不存在、無法讀取、另一個 LMDB build（來自其他發行版的 client）、尚未初始化，或 `EMFOR_MAX_STALENESS` 無效。 | `store /var/lib/emfor/state.mdb does not exist; is the emfor daemon running?` |
| `INCOMPATIBLE` | store 的格式版本既不是 1 也不是 2，或某筆記錄無法解析。 | |
| `NOT_FOUND` | store 中沒有這個服務。 | `no service named "nope.svc.emfor"` |
| `ALL_DOWN` | 沒有 backend 可以選，且 `on_all_down = "closed"`（或每個 backend 都被 drain）。 | `service db.svc.emfor: no backend is healthy, and on_all_down is closed` |

印出時，錯誤的形式是 `CODE: message`。各自的處理方式：

- `ALL_DOWN`：就 emfor 所知，這個服務已經 down。讓請求失敗，或改用備用方案。
- `NOT_FOUND`：通常是打錯字，或這個服務不在這台主機的設定裡。
- `UNAVAILABLE`：daemon 沒有在執行、你的使用者讀不到 store，或 client 來自其他發行版。這是主機的問題，不是服務的問題。

過期的 store 不是錯誤：由 `on_all_down` 決定（[第 8 章](08-choosing-a-backend.md#層級)）。

### 環境

在行程第一次查詢時讀取，`fork` 之後的子行程會再讀一次：

| 變數 | 預設值 | 意義 |
| --- | --- | --- |
| `EMFOR_STORE` | `/var/lib/emfor/state.mdb` | store。`[store] path` 不是預設值時設定它（FreeBSD port 使用 `/var/db/emfor/state.mdb`）。 |
| `EMFOR_MAX_STALENESS` | `3s` | heartbeat 可以多舊：正整數加上 `ms` 或 `s`（`1500ms`、`5s`）。值無效時每次查詢都會失敗：`UNAVAILABLE: EMFOR_MAX_STALENESS="3" is invalid: use a positive whole number followed by ms or s, such as 3s or 1500ms`。 |
| `EMFOR_LIBRARY` | | 只用於 Python：`libemfor` 不在標準目錄時，它的路徑。 |

沒有其他設定。client 從不讀取 daemon 的設定檔。

### 權限

行程需要 store 的讀取權限，以及**它的 lock 檔**（`<store>-lock`）**的寫入權限**，每個讀取者都要在 lock 檔裡登記。使用預設的權限（0640 與 0660，擁有者是 daemon 的使用者與群組）時，請把應用程式的使用者加進 daemon 的群組：

```sh
# usermod -aG emfor www-data
```

不在群組裡的使用者會得到
`UNAVAILABLE: cannot open store /var/lib/emfor/state.mdb: Permission denied`。
見[第 12 章](12-operating.md#誰能讀取-store)。

### 執行緒、fork 與被替換的 store

- 可以從多個執行緒同時呼叫 `lookup`。round-robin 計數器是每個行程各自一份，從隨機的 backend 開始。
- 在 `fork()` 之後的子行程中也能用，即使父行程之前查詢過，或其他執行緒在那一刻正在查詢：子行程會重新開啟 store。
- 一次查詢只在讀取期間占用一個 LMDB reader slot。store 整台主機共有 1024 個 slot。查詢途中被殺掉的行程會留下一個被占用的 slot，直到 daemon 釋放它，大約每分鐘一次。
- daemon 替換 store 檔案時（啟動時把損毀的 store 移到一旁，或檔案被刪除），執行中的應用程式會在大約 100 ms 內切換到新檔案。不需要重新啟動。
- 存在但開不起來的 store 檔案最多每秒重試一次；不存在的 store 每次查詢都會重試，所以比 daemon 先啟動的應用程式，在 store 出現後就能找到它。

不要單獨刪除 `<store>-lock`，並請在以 root 或 daemon 使用者身分執行的應用程式之前啟動 daemon：lock 檔不存在時，第一個開啟 store 的行程會建立它，擁有者是該行程的使用者，以另一個使用者執行的 daemon 之後可能無法啟動。

### 與 daemon 同一發行版

`libemfor` 內含 daemon 自己的 LMDB build，而 lock 檔的格式取決於它：系統的 `liblmdb`、PHP 的 `dba`，或其他 emfor 發行版的 client，都無法可靠地讀取 store。請從與 daemon 相同的發行版安裝 client，並一起升級。版本不符會以 `UNAVAILABLE` 呈現，訊息會說明原因。

### 成本

取自 [docs/performance.md](../performance.md)（Apple M4 Max、Rust client；不是為本手冊量測的）：

| | macOS | Linux（容器） |
| --- | --- | --- |
| 行程中的第一次查詢（開啟 store） | 443 µs | 151 µs |
| 查詢，單一執行緒，中位數 | 1.5 µs | 1.1 µs |
| 對 daemon 的 DNS A 查詢，中位數 | 24.8 µs | 4.6 µs |

Go、Python、Perl 與 PHP 會加上各自的呼叫開銷。吞吐量隨執行緒增加到大約四個為止；之後由 store 共用的 reader lock 決定速度。一次 `sticky` 查詢，每個候選的每一單位權重都要算一次 MD5。所以：每個請求都查詢，不要只在啟動時查一次。快取答案會讓健康檢查失去作用。

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
| `emfor_status emfor_lookup(const char *name, const char *affinity_key, char *out, uintptr_t out_len)` | 把位址與一個 NUL 寫入 `out`。`affinity_key` 可以是 `NULL`。失敗時 `out` 不變。 |
| `const char *emfor_status_name(int status)` | `"OK"`、`"ALL_DOWN"` 等；靜態字串。 |
| `const char *emfor_last_error(void)` | 這個執行緒最近一次失敗呼叫的訊息，在它下一次呼叫 emfor 之前有效。 |

`emfor_status` 是 `EMFOR_OK`（0）、`EMFOR_UNAVAILABLE`（1）、
`EMFOR_INCOMPATIBLE`（2）、`EMFOR_NOT_FOUND`（3）、`EMFOR_ALL_DOWN`（4）或
`EMFOR_INVALID_ARGUMENT`（5：名稱或緩衝區是 `NULL`、字串不是 UTF-8，或緩衝區太小）。`EMFOR_ADDR_MAX`（64）一定放得下。標頭檔也定義了 `EMFOR_MAJOR`、`EMFOR_MINOR` 與 `EMFOR_PATCH`。

如果 `libemfor` 不在 dynamic linker 會搜尋的目錄中，請加上 `-Wl,-rpath,<libdir>` 或執行 `ldconfig`。

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
- 失敗時是 `*emfor.Error`，帶有 `Code`（`emfor.Unavailable`、`emfor.Incompatible`、`emfor.NotFound`、`emfor.AllDown`）與 `Message`。名稱或 key 含 NUL 位元組，或不是有效的 UTF-8 時，得到一般的 error。
- 這個 package 使用 cgo 與 `pkg-config emfor`：它需要 C 編譯器，無法以 `CGO_ENABLED=0` 建置。
- 模組的 tag 是 `clients/go/v0.7.0`。測試時，`go.mod` 以一行 `replace` 指向本機的 checkout。

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
- 錯誤是 `emfor.Error` 的子類別：`UnavailableError`、`IncompatibleError`、`NotFoundError`、`AllDownError`，各自帶有 `.code` 與 `.message`。
- Python 3.9 或更新版本。這個 package 使用 `ctypes`；不需要編譯任何東西。
- 可以從多個執行緒同時使用（呼叫期間會釋放 GIL），在 `os.fork()` 之後也能用。
- 它從 `EMFOR_LIBRARY` 載入 `libemfor`，沒有設定時依名稱從標準目錄載入。

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
- 失敗時以 `Emfor::Error` die：帶有 `code`、`message`，作為字串時是 `"CODE: message"`。參數含 NUL 位元組時，以一個 `Emfor::lookup: invalid argument` 開頭的一般字串 die。
- 參數是字元字串，以 UTF-8 傳遞。位元組要先解碼（`Encode::decode('UTF-8', $bytes)`），否則非 ASCII 的 affinity key 選到的 backend 會與其他語言不同。
- 建置：在 `clients/perl` 中執行 `perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`。

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

請為你執行的每個 PHP minor 版本（PHP 8.1 或更新版本）各建置一次擴充模組，並從原始碼的複本建置，因為 `phpize` 會寫出自己的 Makefile：

```sh
$ mkdir -p /tmp/emfor-php
$ cp clients/php/config.m4 clients/php/emfor.c clients/php/emfor_arginfo.h /tmp/emfor-php/
$ cd /tmp/emfor-php && phpize && ./configure --enable-emfor && make
# make install
```

然後在 PHP 會載入的 ini 檔中加上 `extension=emfor`（`php --ini` 會列出這些檔案）。

- `Emfor\lookup(string $name, ?string $affinity_key = null): string`。
- 失敗時丟出 `Emfor\LookupException`（一個 `RuntimeException`）：`$e->errorCode` 是代碼名稱，`getCode()` 是 C 的狀態值，`getMessage()` 是 `"CODE: message"`。NUL 位元組或無效的 UTF-8 會丟出 `ValueError`。
- 在 `pcntl_fork()` 之後也能用，也能與 `dba` 擴充模組的 `lmdb` handler 並存。
- 使用 PHP-FPM 時，每個 worker 行程都是一個獨立的 client。

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
- `Error` 帶有 `code`（`ErrorCode::Unavailable`、`Incompatible`、`NotFound`、`AllDown`）與訊息；`Display` 給出 `CODE: message`。
- 這個 crate 本身內含 bundled 的 LMDB，所以不需要 `libemfor`。

## 不用 client 函式庫

- **DNS**：任何程式都能透過 daemon 的 DNS server 解析 `web.svc.emfor`（[第 10 章](10-dns.md)）。A 與 AAAA 回答不帶 port。
- **命令列**：`emfor lookup web.svc.emfor` 印出一個位址並以 0 結束，或印出錯誤並以 1（`ALL_DOWN`）、2（沒有這個服務）或 3（store 無法使用）結束。適合 script；每個請求都執行就太慢了。
