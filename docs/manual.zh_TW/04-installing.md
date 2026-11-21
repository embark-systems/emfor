# 4. 安裝

[手冊](README.md) · 上一章：[3. 基本概念](03-concepts.md) · 下一章：[5. 設定 emfor](05-configuring.md)

## 什麼東西裝在哪裡

| 主機 | 安裝 |
| --- | --- |
| 每一台應用程式主機 | daemon `emfor`、一份設定，以及服務定義（systemd unit 或 rc.d script）。 |
| 使用 `emfor-check` 檢查（redis、MySQL、PostgreSQL、LDAP、DNS、RADIUS、SIP、Diameter、SMTP、AMQP、MongoDB、gRPC、QUIC）的主機 | 另外安裝 `emfor-check`。套件一律包含它。 |
| 應用程式使用 client 函式庫的主機 | `libemfor`，以及應用程式所用語言的包裝。 |
| 程式只用 DNS 的主機 | 不需要其他東西。從主機的 resolver 轉送 zone 即可（[第 10 章](10-dns.md#從主機的-resolver-轉送)）。 |

daemon 與每個 client 函式庫都要**從同一個 emfor 發行版**安裝，並一起升級。client 函式庫內含 daemon 自己的 LMDB build；版本不符時，查詢會以 `UNAVAILABLE` 失敗，訊息會說明原因。

## 套件：deb 與 rpm

repository 可以建出 deb 與 rpm。它們**沒有發布**在任何地方，也**沒有簽章**；你要自己在 Docker 裡建置：

```sh
$ packages/build.sh                         # 為這台機器的架構建置
$ PLATFORM=linux/amd64 packages/build.sh    # 為其他架構建置（用模擬時很慢）
```

套件會放在 `packages/dist/`，檔名為 `emfor_<version>-1_<arch>.deb` 與 `emfor-<version>-1.<arch>.rpm`。

執行檔以 glibc 2.31 建置，所以能在 Debian 11 及更新版、Ubuntu 20.04 及更新版，以及 RHEL 9 與相近的發行版上執行。

套件安裝的東西：

| 路徑 | 內容 |
| --- | --- |
| `/usr/bin/emfor`、`/usr/bin/emfor-check` | daemon 與 plugin 程式。預設設定檔路徑為 `/etc/emfor/emfor.toml`。 |
| `/usr/lib/systemd/system/emfor.service` | 加固過的 systemd unit（[見下](#systemd-unit)）。 |
| `/etc/emfor/emfor.toml` | 一份只設定 `include_dir = "conf.d"` 的設定檔。升級時保留。 |
| `/etc/emfor/conf.d/` | 你的服務檔案放在這裡；有一份 `README` 說明。 |
| `/usr/share/doc/emfor/` | 附註解的範例設定（deb 裡還有舊版手冊與 Prometheus 規則）。 |

安裝 script 會建立使用者與群組 `emfor`，並讓 `/etc/emfor` 與 `/etc/emfor/conf.d` 的擁有者為 root、權限 0755。store 目錄 `/var/lib/emfor` 由 unit 建立（`StateDirectory=emfor`）。

```sh
# dpkg -i emfor_0.7.0-1_amd64.deb      # Debian、Ubuntu：立刻啟用並啟動
# rpm -i emfor-0.7.0-1.x86_64.rpm      # RHEL 與相近的發行版：不會啟用
# vi /etc/emfor/conf.d/10-web.toml     # 你的服務
# emfor check-config
# systemctl enable --now emfor         # rpm；deb 則用：systemctl reload emfor
# emfor status
```

- **deb**：`dpkg -r` 保留設定；`dpkg -P` 移除設定與 `/var/lib/emfor`。使用者會保留。
- **rpm**：升級會重新啟動執行中的 daemon。從 0.7.0 起，rpm 把 `/etc/emfor/emfor.toml` 標為 `%config(noreplace)`：升級時保留你改過的檔案，並把套件附的版本寫在旁邊，名為 `emfor.toml.rpmnew`。

兩個套件都依賴 `ca-certificates`，驗證憑證的 `https` 檢查需要它。

測試過的範圍：deb 與 rpm 在 arm64 上的 Debian 12 與 Rocky Linux 9 container 中（安裝、移除、purge、在改過的設定上升級），以及 deb 在 Debian 12 container 中以 systemd 252 作為 PID 1 執行。**沒有測試**：rpm 在實體主機上以 systemd 長時間執行的服務、x86-64 套件、其他發行版。見 [packages/README.md](../../packages/README.md)。

## 從原始碼建置

需要 Rust 1.85 或更新（2024 edition）。在 checkout 或解開的原始碼發行版中：

```sh
$ cargo build --release --bin emfor --bin emfor-check
# install -m 0755 target/release/emfor target/release/emfor-check /usr/bin/
```

`emfor-check` 是選用的；只有使用它的檢查時才需要安裝。資料庫驅動程式只連結進 `emfor-check`，不在 daemon 或 client 函式庫裡。

daemon 不會自己去找 `emfor-check`：plugin 的 `command` 寫明它的路徑（[第 7 章](07-plugins.md)）。它在哪裡，取決於你怎麼安裝：

| 安裝方式 | `emfor-check` |
| --- | --- |
| deb 與 rpm 套件 | `/usr/bin/emfor-check` |
| FreeBSD port | `/usr/local/bin/emfor-check` |
| 從原始碼，用上面的 `install` 那一行 | `/usr/bin/emfor-check` |
| 從原始碼，用 `cargo install --root /usr/local`（[下面](#預設設定檔路徑)） | `/usr/local/bin/emfor-check` |

### 預設設定檔路徑

`emfor` 讀取 `/etc/emfor/emfor.toml`，除非 `--config` 指定其他檔案。預設值在建置時由環境變數 `EMFOR_PREFIX` 決定：

| `EMFOR_PREFIX` | 預設設定檔路徑 |
| --- | --- |
| 未設定、空字串、`/` 或 `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |

```sh
$ EMFOR_PREFIX=/usr/local cargo build --release --bin emfor
```

相對路徑的 prefix 會讓建置失敗。`emfor --help` 會顯示內建的路徑。只有設定檔路徑跟著 prefix 改變：store 仍在 `[store] path`（預設 `/var/lib/emfor/state.mdb`），這也是 client 去找的位置。

使用 `cargo install` 時，要指定每個 crate（repository 根目錄是 workspace）：

```sh
# EMFOR_PREFIX=/usr/local cargo install --locked --path crates/emfor-daemon --root /usr/local
# cargo install --locked --path crates/emfor-check --root /usr/local
```

## systemd unit

套件會安裝 unit；從原始碼安裝時，複製 [examples/systemd/emfor.service](../../examples/systemd/emfor.service)，它執行 `/usr/local/bin/emfor`（如果你裝到 `/usr/bin`，修改其中三個路徑）：

```sh
# useradd --system --no-create-home --shell /usr/sbin/nologin emfor
# install -m 0644 examples/systemd/emfor.service /etc/systemd/system/emfor.service
# install -d -m 0755 /etc/emfor /etc/emfor/conf.d
# install -m 0644 my-emfor.toml /etc/emfor/emfor.toml
# emfor check-config
# systemctl daemon-reload
# systemctl enable --now emfor
```

unit 做的事：

| 設定 | 效果 |
| --- | --- |
| `User=emfor`、`Group=emfor` | daemon 以非特權身分執行。 |
| `ExecStartPre=... check-config` | 無效的設定絕不會啟動。 |
| `ExecReload=... check-config` 然後 `kill -HUP` | `systemctl reload emfor` 只在設定有效時才送 signal 給 daemon。 |
| `Restart=on-failure`、`RestartPreventExitStatus=1 2` | 當掉（包括結束代碼 70）會被重新啟動；結束代碼 1（無法啟動）與 2（設定無效）不會，因為重試也無法解決。 |
| `StartLimitIntervalSec=60`、`StartLimitBurst=5` | 一分鐘內啟動失敗五次後，unit 會處於 `failed`，直到執行 `systemctl reset-failed emfor`。0.7.0 新增。 |
| `StateDirectory=emfor`、`StateDirectoryMode=0755` | 為 store 建立 `/var/lib/emfor`，擁有者為 `emfor`。 |
| `UMask=0027`、`ProtectProc=invisible`、`ProcSubset=pid` | 0.7.0 新增。不論如何，store 的檔案都會自己設定權限。 |
| `ProtectSystem=strict`、`NoNewPrivileges=yes`、`SystemCallFilter=@system-service` 等 | 加固。plugin 與 `exec` 檢查也在同樣的限制下執行。 |
| `LimitNOFILE=65536` | 檢查、plugin 與 DNS 連線都會用到 file descriptor。 |
| `CapabilityBoundingSet=` | 沒有任何 capability。DNS 要用 53 port 時，改為 `CapabilityBoundingSet=CAP_NET_BIND_SERVICE` 與 `AmbientCapabilities=CAP_NET_BIND_SERVICE`。 |

需要知道的事：

- 除了 `/var/lib/emfor`，檔案系統對 daemon 都是唯讀。`[store] path` 放在別處時，需要在 drop-in 中加上 `ReadWritePaths=`（`systemctl edit emfor`）。
- 不要用 `DynamicUser=`：它的 state 目錄應用程式無法存取。
- `MemoryDenyWriteExecute=yes` 以註解形式寫在 unit 裡。如果你的 plugin 都不在執行期產生程式碼，可以打開它；它在 systemd 252 上搭配 daemon 與 `dns` plugin 試過。
- log 送進 journal：`journalctl -u emfor`。
- 為舊版製作的範例 unit 複本不會自己改變：升級時拿它和新版比較（[第 15 章](15-upgrading.md)）。

## FreeBSD

[packages/freebsd/](../../packages/freebsd/) 是一個範例 port，`net-mgmt/emfor`。它以 `EMFOR_PREFIX=/usr/local` 建置兩個程式、安裝 rc.d script 與範例設定檔，並建立 `emfor` 使用者、空的 `/usr/local/etc/emfor/conf.d`，以及給 store 用的 `/var/db/emfor`。

```sh
# sysrc emfor_enable=YES
# service emfor start
# emfor status
```

| rc.conf 變數 | 預設值 | 意義 |
| --- | --- | --- |
| `emfor_enable` | `NO` | 開機時啟動。 |
| `emfor_config` | `/usr/local/etc/emfor/emfor.toml` | 設定檔。 |
| `emfor_user` | `emfor` | daemon 執行時的使用者。需要 store 所在目錄的寫入權限。 |

- `service emfor configtest` 檢查設定，`service emfor reload` 先檢查再送 SIGHUP，`start` 拒絕無效的設定。
- daemon 在 `daemon(8)` 底下執行，daemon 結束 2 秒後 `daemon(8)` 會再把它啟動。它的 log 以 tag `emfor` 送進 syslog。
- plugin 程式是 `/usr/local/bin/emfor-check`，不是 `/usr/bin`。
- 範例設定檔把 store 放在 `/var/db/emfor/state.mdb`，不是預設位置。請給主機上的每個應用程式設定 `EMFOR_STORE=/var/db/emfor/state.mdb`。

這個 port 已在 FreeBSD 15.1-RELEASE（arm64，在虛擬機器中）上以 `make stage check-plist install` 建置並安裝過。尚未用 poudriere 跑過，也沒有在 13.x、14.x，或專案 CI 以外的 amd64 上跑過。

## client 函式庫

每個 client 都透過 `libemfor` 讀取 store，它以 [cargo-c](https://github.com/lu-zero/cargo-c) 從 [clients/rust](../../clients/rust/README.md) 建置。它內含 emfor 自己的 LMDB build；系統的 liblmdb 無法讀取 store。

```sh
$ cargo install cargo-c
$ cd clients/rust
# cargo cinstall --release --features capi --prefix /usr/local
```

這會安裝 `libemfor.so`（0.7.x 的 soname 為 `libemfor.so.0.7`）、`libemfor.a`、標頭檔 `emfor.h`，以及給 `pkg-config` 用的 `emfor.pc`。

| 語言 | 安裝 |
| --- | --- |
| Rust | 依賴同一發行版的 `emfor-client` crate（`clients/rust`）。不需要 `libemfor`。 |
| C | 照上面安裝 `libemfor`；以 `cc app.c $(pkg-config --cflags --libs emfor)` 建置。 |
| Go | 安裝 `libemfor`，然後 `go get github.com/embark-systems/emfor/clients/go@v0.7.0`。需要 cgo 與 `pkg-config emfor`。 |
| Python | 安裝 `libemfor`，然後 `pip install clients/python`（Python 3.9 或更新；它使用 `ctypes`）。函式庫不在標準目錄時，把 `EMFOR_LIBRARY` 設為函式庫的路徑。 |
| Perl | 安裝 `libemfor`，然後在 `clients/perl` 執行：`perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`。 |
| PHP | 安裝 `libemfor`，然後從 `clients/php` 的複本用 `phpize` 建置 extension（見[第 9 章](09-clients.md#php)），並啟用 `extension=emfor`。PHP 8.1 或更新。 |

優先使用 shared library：它只匯出 `emfor_*` 符號，所以它的 LMDB 不會與同一行程裡的其他 LMDB 衝突。

[第 9 章](09-clients.md)說明各 client 的用法。
