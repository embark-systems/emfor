# 2. 安裝

[手冊](README.md) · 上一章：[1. 基本概念](01-concepts.md) · 下一章：[3. 設定](03-configuration.md)

emfor 用 Cargo 從原始碼建置。Linux 與 FreeBSD 是部署平台；macOS 可用於開發。

## 什麼東西裝在哪裡

| 主機 | 安裝 |
| --- | --- |
| 每一台應用程式主機 | daemon `emfor`、它的設定檔，以及服務定義（systemd 或 rc.d）。 |
| 需要檢查 redis、MySQL、PostgreSQL 或 LDAP 的主機 | 另外安裝 `emfor-check`。 |
| 應用程式使用 client 函式庫的主機 | `libemfor`，以及應用程式所用語言的包裝。 |
| 程式只用 DNS 的主機 | 不需要其他東西；從主機的 resolver 轉送 zone 即可（[第 7 章](07-dns.md)）。 |

## 建置 daemon

需要支援 2024 edition 的 Rust 工具鏈（Rust 1.85 或更新）。在 checkout 或解開的原始碼發行版中：

```sh
cargo build --release --bin emfor --bin emfor-check
install -m 0755 target/release/emfor /usr/local/bin/emfor
install -m 0755 target/release/emfor-check /usr/local/libexec/emfor-check
```

`emfor-check` 是選用的；只有使用它的 plugin 時才需要安裝（[第 4 章](04-health-checks.md#plugin-與-emfor-check)）。資料庫驅動程式只連結進 `emfor-check`，不在 daemon 或 client 函式庫裡。

### 預設設定檔路徑

`emfor` 讀取 `/etc/emfor/emfor.toml`，除非 `--config` 指定其他檔案。預設值在建置時由環境變數 `EMFOR_PREFIX` 決定：

| `EMFOR_PREFIX` | 預設設定檔路徑 |
| --- | --- |
| 未設定、空字串或 `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |

```sh
EMFOR_PREFIX=/usr/local cargo build --release --bin emfor
```

prefix 必須是絕對路徑，否則建置失敗。`emfor --help` 會顯示內建的路徑。只有設定檔路徑跟著 prefix 改變：store 仍在設定檔的 `[store] path`（預設 `/var/lib/emfor/state.mdb`），這也是 client 預設去找的位置。

### 使用 cargo install

repository 根目錄是 workspace 而不是 package，所以要指定每個 crate：

```sh
EMFOR_PREFIX=/usr/local cargo install --locked --path crates/emfor-daemon --root /usr/local
cargo install --locked --path crates/emfor-check --root /usr/local
```

`--root` 只決定執行檔放在哪裡；`EMFOR_PREFIX` 也要另外設定。

## 在 systemd 下執行

repository 裡有一個加固過的 unit：[examples/systemd/emfor.service](../../examples/systemd/emfor.service)。它以使用者 `emfor` 執行 `/usr/local/bin/emfor server --config /etc/emfor/emfor.toml`：

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin emfor
install -m 0644 examples/systemd/emfor.service /etc/systemd/system/emfor.service
install -d /etc/emfor
install -m 0644 examples/emfor.toml /etc/emfor/emfor.toml   # then edit it
emfor check-config
systemctl daemon-reload
systemctl enable --now emfor
emfor status
```

這個 unit 替你處理的事：

- `StateDirectory=emfor` 為 store 建立 `/var/lib/emfor`，擁有者為 `emfor`。不要改用 `DynamicUser=`：它的目錄應用程式無法存取。
- `systemctl reload emfor` 會先執行 `emfor check-config`，設定有效時才送出 SIGHUP。
- daemon 當掉後會被重新啟動，包括結束代碼 70；但結束代碼 1 或 2 不會，因為重試也無法解決（[結束代碼](08-operations.md#結束代碼)）。
- log 送進 journal：`journalctl -u emfor`。
- 除了 `/var/lib/emfor`，檔案系統都是唯讀。`[store] path` 放在別處時需要 `ReadWritePaths=`；DNS 用 53 port 時需要 `CAP_NET_BIND_SERVICE`（見 [docs/deploy/daemon.md](../deploy/daemon.md#running-under-systemd)）。

建置時不要設定 `EMFOR_PREFIX`（或設為 `/usr`），這樣 `emfor status` 與 `emfor check-config` 不用 `--config` 也能找到 `/etc/emfor/emfor.toml`。

## 在 FreeBSD 上執行

[examples/rc.d/emfor](../../examples/rc.d/emfor) 是一個 rc.d script，在 `daemon(8)` 底下執行 daemon；daemon 結束 2 秒後 `daemon(8)` 會再把它啟動。把它複製到 `/usr/local/etc/rc.d/emfor`（mode 0555），以 `EMFOR_PREFIX=/usr/local` 建置 emfor，然後：

```sh
sysrc emfor_enable=YES
sysrc emfor_user=emfor        # recommended; see the script's comments
service emfor start
emfor status
```

| rc.conf 變數 | 預設值 | 意義 |
| --- | --- | --- |
| `emfor_enable` | `NO` | 開機時啟動。 |
| `emfor_config` | `/usr/local/etc/emfor/emfor.toml` | 設定檔。 |
| `emfor_user` | 見 script | daemon 執行時的使用者。需要 store 所在目錄的寫入權限。 |

`service emfor configtest` 檢查設定檔（以 `emfor_user` 身分）；`service emfor reload` 先檢查再送 SIGHUP；`start` 拒絕無效的設定。daemon 的 log 以 tag `emfor` 送進 syslog。

[examples/freebsd/](../../examples/freebsd/) 是一個範例 port（`net-mgmt/emfor`），它建置 `emfor` 與 `emfor-check`、安裝 rc.d script 與範例設定檔，並建立 `emfor` 使用者與 `/var/db/emfor`，它的範例設定檔把 store 放在那裡（應用程式要設定 `EMFOR_STORE=/var/db/emfor/state.mdb`）。使用前請先讀 script 的註解與 port 的 README。兩者都已在 FreeBSD 15.1-RELEASE（arm64，VM）上跑過，port 用的是 `make stage check-plist install`；尚未用 poudriere、14.x 或 amd64 測試。

## 安裝 client 函式庫

每個 client 都透過 `libemfor` 讀取 store，它由 [clients/rust](../../clients/rust/README.md) 建置而來，內含 daemon 自己的 LMDB build：系統的 liblmdb 無法讀取 store。

**client 函式庫必須與 daemon 來自同一個 emfor 發行版，並一起升級。** 版本不符時，查詢會以 `UNAVAILABLE` 失敗，訊息會說明原因。

| 語言 | 方式 |
| --- | --- |
| Rust | 依賴同一發行版的 `emfor-client` crate（`clients/rust`）。 |
| C | `cargo install cargo-c`，然後在 `clients/rust` 執行 `cargo cinstall --release --features capi --prefix /usr/local`。這會安裝 `libemfor`、`emfor.h` 與 `emfor.pc`。 |
| Go | 安裝 `libemfor`，然後 `go get github.com/embark-systems/emfor/clients/go@v0.2.0`（需要 cgo 與 `pkg-config emfor`）。 |
| Python | 安裝 `libemfor`，然後 `pip install clients/python`。函式庫不在標準目錄時，設定 `EMFOR_LIBRARY`。 |
| Perl | 安裝 `libemfor`，然後在 `clients/perl` 執行 `perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`。 |
| PHP | 安裝 `libemfor`，然後用 `phpize` 建置 extension（見 [clients/php](../../clients/php/README.md)），並啟用 `extension=emfor`。 |

優先使用 shared library：它只匯出 `emfor_*` 符號，所以內含的 LMDB 不會與同一行程裡的其他 LMDB 衝突。[第 6 章](06-clients.md)說明各 client 的用法。
