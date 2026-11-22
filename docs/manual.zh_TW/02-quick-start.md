# 2. 快速上手

[手冊](README.md) · 上一章：[1. emfor 是什麼](01-what-emfor-is.md) · 下一章：[3. 基本概念](03-concepts.md)

花十分鐘，在一台 Linux 主機上，你會：

1. 啟動兩個小型 web server 當 backend；
2. 讓 emfor 檢查它們；
3. 從 shell、透過 DNS、從 Python 問 emfor 該用哪一個；
4. 停掉一個 backend，看 emfor 把它拿掉。

你需要一台 Linux 機器的 root 權限（虛擬機器或 container 都可以），上面要有 `python3` 與 `curl`。`dig` 是選用的。指令用的是 `127.0.0.1` 與 `127.0.0.2` 這兩個位址，每個 Linux 系統的 loopback 介面上都有。（在 FreeBSD 上，要先執行 `ifconfig lo0 alias 127.0.0.2/32` 才有 `127.0.0.2`；這份逐步說明只在 Linux 上跑過。）

## 步驟 1：安裝 emfor

兩種方式選一種：

- **用 deb 或 rpm 套件。** 專案不發布套件，要你自己在 checkout 裡用 `packages/build.sh` 建出來（需要 Docker，見[第 4 章](04-installing.md#套件deb-與-rpm)），再裝適合你系統的那一個。deb 一裝好就會啟動 daemon，這時還沒有任何服務。
- **從原始碼。** 用 Rust 工具鏈（1.85 或更新）：

  ```sh
  $ cargo build --release --bin emfor --bin emfor-check
  # install -m 0755 target/release/emfor target/release/emfor-check /usr/bin/
  ```

確認它能跑：

```console
$ emfor --version
emfor 0.7.0
```

## 步驟 2：啟動兩個 backend

開第二個終端機，啟動兩個 web server，port 相同，位址各用一個：

```sh
$ mkdir -p /tmp/web1 /tmp/web2
$ echo "hello from web1" > /tmp/web1/index.html
$ echo "hello from web2" > /tmp/web2/index.html
$ python3 -m http.server 8080 --bind 127.0.0.1 --directory /tmp/web1 &
$ python3 -m http.server 8080 --bind 127.0.0.2 --directory /tmp/web2 &
```

## 步驟 3：描述服務

**服務**就是一個名稱加上一份 **backend** 清單。這個服務叫 `web.svc.emfor`，emfor 用 HTTP `GET /` 檢查它。

**如果你是從原始碼建置的**，先建好目錄（daemon 不會自己建），再把下面的內容寫進 `/etc/emfor/emfor.toml`：

```sh
# install -d -m 0755 /etc/emfor /var/lib/emfor
```

```toml
# /etc/emfor/emfor.toml
[dns]        # 在 127.0.0.1:5300 回答 DNS 查詢
[metrics]    # 在 127.0.0.1:5301 提供 Prometheus metrics

[services."web.svc.emfor"]
check = "http"
backends = ["127.0.0.1:8080", "127.0.0.2:8080"]

[services."web.svc.emfor".check_options]
path = "/"
```

```sh
# chmod 0644 /etc/emfor/emfor.toml
```

設定檔不能讓 group 或其他人寫入，而且 store 目錄 `/var/lib/emfor` 一定要先存在（上面那行指令已經建好了）。

**如果你裝的是套件**，`/etc/emfor/emfor.toml` 不要動（它會讀 `/etc/emfor/conf.d/` 裡的每個 `.toml` 檔），改把同樣的內容寫進 `/etc/emfor/conf.d/10-web.toml`，權限設成 0644。

現在讓 emfor 檢查這個檔案：

```console
# emfor check-config
config ok: 1 services, 2 backends
```

用套件的話，`check-config` 也會印出它讀了哪些檔案。

## 步驟 4：啟動 daemon

**從原始碼**裝的，在第三個終端機用前景模式跑它（log 會直接印在那裡）：

```console
# emfor server
INFO emfor_daemon::cli: DNS server for zone svc.emfor on 127.0.0.1:5300
INFO emfor_daemon::cli: metrics on 127.0.0.1:5301
INFO emfor_daemon::cli: emfor started: 1 services, 2 backends, store /var/lib/emfor/state.mdb
INFO emfor_daemon::schedule: backend 127.0.0.1:8080 of web.svc.emfor changed from unknown to healthy (check passed) ...
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from unknown to healthy (check passed) ...
```

（每一行 log 開頭都有時間戳記，這裡省略了。）

**用套件的話**，daemon 已經在跑（deb），或要你自己啟動（rpm 用 `systemctl enable --now emfor`）。用 `systemctl reload emfor` 載入新檔案，用 `journalctl -u emfor` 看 log。

## 步驟 5：問 emfor

`emfor status` 會顯示 daemon 發布了什麼：

```console
# emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.0s ago, up 2.0s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy  1.7s ago  8.4ms
  127.0.0.2:8080  healthy  1.6s ago  7.5ms
```

`emfor lookup` 會印出應用程式會拿到的位址。這個服務用 `round-robin`，所以跑兩次可能會印出不同的 backend：

```console
# emfor lookup web.svc.emfor
127.0.0.1:8080
# curl "http://$(emfor lookup web.svc.emfor)/"
hello from web2
```

DNS server 不管哪個程式來問，都給一樣的答案：

```console
$ dig +short @127.0.0.1 -p 5300 web.svc.emfor A
127.0.0.1
$ dig +short @127.0.0.1 -p 5300 web.svc.emfor SRV
0 1 8080 127-0-0-1.web.svc.emfor.
0 1 8080 127-0-0-2.web.svc.emfor.
```

A 記錄不帶 port；SRV 記錄會列出每個健康的 backend 和它的 port。

上面這些指令都是用 root 跑的。換成一般使用者，`emfor status` 與 `emfor lookup` 會失敗，錯誤是 `Permission denied`：從 0.7.0 起，只有 daemon 的使用者與群組能讀 store（見[第 12 章](12-operating.md#誰能讀取-store)）。用套件的話，把你的使用者加進 `emfor` 群組（`usermod -aG emfor $USER`，然後重新登入）。

## 步驟 6：弄壞一個 backend

停掉第二個 web server：

```sh
$ kill %2
```

大約六秒後（三次檢查失敗，每次間隔兩秒），emfor 會把它標成 unhealthy：

```console
# emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.0s ago, up 9.0s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    1.0s ago  7.2ms
  127.0.0.2:8080  unhealthy  1.0s ago  -      error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
```

現在每次查詢都會回傳第一個 backend。加上 `--explain` 可以看原因：

```console
# emfor lookup web.svc.emfor --explain
service: web.svc.emfor
answering: as a client library answers
store: /var/lib/emfor/state.mdb (heartbeat 0.0s ago, fresh)
balance: round-robin  on_all_down: open
tier: healthy primary backends whose last check passed
candidates:
  127.0.0.1:8080
excluded:
  127.0.0.2:8080  unhealthy
selection: round-robin: the candidates in turn
  the actual choice depends on the rotation start of the process, which is random in a new process
answer: 127.0.0.1:8080
```

把這個 server 再啟動起來（`python3 -m http.server 8080 --bind 127.0.0.2 --directory /tmp/web2 &`）。通過兩次檢查後它又會變回 `healthy`，用 `emfor status --history` 可以看到這兩次變化：

```console
# emfor status --history --service web.svc.emfor
...
web.svc.emfor
  127.0.0.1:8080
    2026-11-21 17:17:33 unknown -> healthy
  127.0.0.2:8080
    2026-11-21 17:17:45 unhealthy -> healthy
    2026-11-21 17:17:39 healthy -> unhealthy  error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
    2026-11-21 17:17:33 unknown -> healthy
```

## 步驟 7：從程式查詢

應用程式用的是 client 函式庫，它需要 `libemfor`（見[第 4 章](04-installing.md#client-函式庫)）。用 Python 的話：

```python
import emfor

addr = emfor.lookup("web.svc.emfor")   # "127.0.0.1:8080"
```

每次建立新連線之前都呼叫 `lookup`，不要只在啟動時呼叫一次：答案會跟著檢查結果變，而一次查詢只要大約一微秒。各種語言的寫法見[第 9 章](09-clients.md)。

## 步驟 8：清理

用 Ctrl-C 停掉 `emfor server`（或 `systemctl stop emfor`），再用 `kill %1 %2` 停掉兩個 web server。store 會留在 `/var/lib/emfor`；每次啟動都會重建它，所以這個目錄可以直接刪掉。

## 接下來讀什麼

- [第 3 章](03-concepts.md)解釋你剛才看到的東西：健康狀態、store，以及查詢從哪些層級裡挑選。
- [第 5 章](05-configuring.md)與[第 6 章](06-health-checks.md)教你怎麼寫正式的設定。
- [第 4 章](04-installing.md)教你正式安裝 emfor，讓它在 systemd 底下執行。
