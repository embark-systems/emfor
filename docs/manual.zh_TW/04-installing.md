# 4. 安裝

[手冊](README.md) · 上一章：[3. 基本概念](03-concepts.md) · 下一章：[5. 設定 emfor](05-configuring.md)

## 什麼東西裝在哪裡

| 主機 | 安裝 |
| --- | --- |
| 每一台應用程式主機 | daemon `emfor`、一份設定，以及服務定義（systemd unit 或 rc.d script）。 |
| 用到 `emfor-check` 檢查（redis、MySQL、PostgreSQL、LDAP、DNS、RADIUS、SIP、Diameter、SMTP、AMQP、MongoDB、gRPC、QUIC）的主機 | 再加裝 `emfor-check`。套件裡一定有它。 |
| 應用程式用 client 函式庫的主機 | `libemfor`，以及應用程式所用語言的包裝。 |
| 程式只用 DNS 的主機 | 不用再裝別的。從主機的 resolver 轉送 zone 就好（[第 10 章](10-dns.md#從主機的-resolver-轉送)）。 |

daemon 和每個 client 函式庫都要裝**同一個 emfor 發行版**，升級時也要一起升。client 函式庫內含 daemon 自己的 LMDB build；版本對不上時，查詢會失敗並回傳 `UNAVAILABLE`，訊息裡會說明原因。

## 套件：deb 與 rpm

repository 可以建出 deb 與 rpm。這些套件**沒有發布**到任何地方，也**沒有簽章**，你要自己在 Docker 裡建：

```sh
$ packages/build.sh                         # 建這台機器的架構
$ PLATFORM=linux/amd64 packages/build.sh    # 建其他架構（要模擬時很慢）
```

建好的套件會放在 `packages/dist/`，檔名是 `emfor_<version>-1_<arch>.deb` 與 `emfor-<version>-1.<arch>.rpm`。

執行檔是對 glibc 2.31 建的，所以 Debian 11 以後、Ubuntu 20.04 以後，以及 RHEL 9 與相近的發行版都能跑。

套件會裝這些東西：

| 路徑 | 內容 |
| --- | --- |
| `/usr/bin/emfor`、`/usr/bin/emfor-check` | daemon 與 plugin 程式。預設設定檔路徑是 `/etc/emfor/emfor.toml`。 |
| `/usr/lib/systemd/system/emfor.service` | 加固過的 systemd unit（[見下](#systemd-unit)）。 |
| `/etc/emfor/emfor.toml` | 只設了 `include_dir = "conf.d"` 的設定檔。升級時會保留。 |
| `/etc/emfor/conf.d/` | 你的服務檔案放這裡；裡面有一份 `README` 說明。 |
| `/usr/share/doc/emfor/` | 附註解的範例設定、Prometheus 規則（在 `examples/prometheus/` 裡）和手冊（在 `manual/` 裡）。 |

安裝 script 會建立使用者與群組 `emfor`，並把 `/etc/emfor` 與 `/etc/emfor/conf.d` 設成 root 擁有、權限 0755。store 目錄 `/var/lib/emfor` 則由 unit 建立（`StateDirectory=emfor`）。

```sh
# dpkg -i emfor_0.8.0-1_amd64.deb      # Debian、Ubuntu：裝好就啟用並啟動
# rpm -i emfor-0.8.0-1.x86_64.rpm      # RHEL 與相近的發行版：不會自動啟用
# vi /etc/emfor/conf.d/10-web.toml     # 你的服務
# emfor check-config
# systemctl enable --now emfor         # rpm；deb 則用：systemctl reload emfor
# emfor status
```

- **deb**：`dpkg -r` 會留下設定；`dpkg -P` 會連設定和 `/var/lib/emfor` 一起刪掉。使用者不會刪。
- **rpm**：升級時會重新啟動執行中的 daemon。從 0.7.0 起，rpm 把 `/etc/emfor/emfor.toml` 標成 `%config(noreplace)`：升級時保留你改過的檔案，套件附的版本則寫在旁邊，檔名是 `emfor.toml.rpmnew`。

兩種套件都依賴 `ca-certificates`，因為有驗證憑證的 `https` 檢查需要它。

**不支援 RHEL 8 與它的衍生版。** 執行檔需要 glibc 2.30，RHEL 8 只有 2.28；請用 RHEL 9 或更新（deb 則是 Debian 11 以上）。

測試過的範圍：deb 與 rpm 在 arm64 上的 Debian 12 與 Rocky Linux 9 container 裡（安裝、移除、purge、在改過的設定上升級）、deb 在 Debian 12 container 裡以 systemd 252 當 PID 1 執行，以及 rpm 在 Rocky Linux 9 container 裡以 systemd 當 PID 1 執行（啟動、reload、停止、`rpm -V`、移除後重裝、從上一版升級再降回去）。**沒有測試過**：在實體主機上執行（container 不算）、x86-64 套件、其他發行版。rpm 另外在 SELinux enforcing 的 Rocky Linux 9.9 虛擬機器上跑過（啟動、檢查、reload，沒有拒絕），deb 也在 AppArmor 啟用的 Ubuntu 24.04 上跑過（沒有拒絕），見[第 13 章](13-security.md#selinux-與-apparmor)。見 [packages/README.md](../../packages/README.md)。

## 沒有網路的環境

daemon 跑起來之後，除了它要檢查的位址，不需要任何網路：backend 是 IP 位址，沒有更新檢查、沒有 telemetry，`http` 檢查也不走 proxy。難的是安裝，不是執行。

- **在有網路的地方建置，在沒網路的地方安裝。** 在連得上網的機器用 `packages/build.sh` 建出 deb 或 rpm（[上面](#套件deb-與-rpm)），把檔案複製過去，用 `dpkg -i` 或 `rpm -i` 安裝。複製後請用 `sha256sum` 對照建置時記下的值；套件沒有簽章。
- **兩種套件都依賴 `ca-certificates`。** 請從你內部的 mirror 取得。伺服器的憑證如果是私有 CA 簽的，請在檢查裡用 `ca_file` 指到那個 CA，不必裝進整個系統（[第 6 章](06-health-checks.md)）。
- **沒有網路時從原始碼建置**就是一般的 cargo：在連網的機器跑 `cargo vendor`，把目錄和 `Cargo.lock` 複製過去，用 `--locked --offline` 建置。專案還沒有提供 vendored 的原始碼樹、材料清單（SBOM）或簽章的發行版，這份離線建置也沒有為這份手冊實際跑過。
- **文件：** 套件會把這份手冊裝在 `/usr/share/doc/emfor/manual/`，所以主機上就有。

## 從原始碼建置

需要 Rust 1.85 或更新版本（2024 edition）。在 checkout 或解開的原始碼發行版裡執行：

```sh
$ cargo build --release --bin emfor --bin emfor-check
# install -m 0755 target/release/emfor target/release/emfor-check /usr/bin/
```

`emfor-check` 是選用的，有用到它的檢查才需要裝。資料庫驅動程式只連結進 `emfor-check`，daemon 和 client 函式庫裡都沒有。

daemon 不會自己到處找 `emfor-check`：plugin 的 `command` 會寫明它的路徑（[第 7 章](07-plugins.md)）。它裝在哪裡，要看你怎麼安裝：

| 安裝方式 | `emfor-check` |
| --- | --- |
| deb 與 rpm 套件 | `/usr/bin/emfor-check` |
| FreeBSD port | `/usr/local/bin/emfor-check` |
| 從原始碼，用上面那行 `install` | `/usr/bin/emfor-check` |
| 從原始碼，用 `cargo install --root /usr/local`（[見下](#預設設定檔路徑)） | `/usr/local/bin/emfor-check` |

### 預設設定檔路徑

`emfor` 預設讀 `/etc/emfor/emfor.toml`，除非你用 `--config` 指定別的檔案。這個預設值在建置時由環境變數 `EMFOR_PREFIX` 決定：

| `EMFOR_PREFIX` | 預設設定檔路徑 |
| --- | --- |
| 沒設、空字串、`/` 或 `/usr` | `/etc/emfor/emfor.toml` |
| `/usr/local` | `/usr/local/etc/emfor/emfor.toml` |
| `/opt/emfor` | `/opt/emfor/etc/emfor/emfor.toml` |

```sh
$ EMFOR_PREFIX=/usr/local cargo build --release --bin emfor
```

prefix 是相對路徑時，建置會失敗。`emfor --help` 會顯示建進去的路徑。只有設定檔路徑會跟著 prefix 改：store 還是在 `[store] path`（預設 `/var/lib/emfor/state.mdb`），client 也是到這裡找。

用 `cargo install` 時，每個 crate 都要分別指定（repository 根目錄是一個 workspace）：

```sh
# EMFOR_PREFIX=/usr/local cargo install --locked --path crates/emfor-daemon --root /usr/local
# cargo install --locked --path crates/emfor-check --root /usr/local
```

## systemd unit

套件會裝好 unit。從原始碼安裝的話，複製 [examples/systemd/emfor.service](../../examples/systemd/emfor.service)；它執行的是 `/usr/local/bin/emfor`（如果你裝在 `/usr/bin`，要改裡面的三個路徑）：

```sh
# useradd --system --no-create-home --shell /usr/sbin/nologin emfor
# install -m 0644 examples/systemd/emfor.service /etc/systemd/system/emfor.service
# install -d -m 0755 /etc/emfor /etc/emfor/conf.d
# install -m 0644 my-emfor.toml /etc/emfor/emfor.toml
# emfor check-config
# systemctl daemon-reload
# systemctl enable --now emfor
```

unit 做了哪些事：

| 設定 | 效果 |
| --- | --- |
| `User=emfor`、`Group=emfor` | daemon 用非特權身分執行。 |
| `ExecStartPre=... check-config` | 設定無效就絕不會啟動。 |
| `ExecReload=... check-config` 然後 `kill -HUP` | `systemctl reload emfor` 只有在設定有效時才送 signal 給 daemon。 |
| `Restart=on-failure`、`RestartPreventExitStatus=1 2` | 當掉時（包括結束代碼 70）會自動重新啟動；結束代碼 1（無法啟動）與 2（設定無效）不會，因為重試也解決不了。 |
| `StartLimitIntervalSec=60`、`StartLimitBurst=5` | 一分鐘內啟動失敗五次後，unit 會停在 `failed`，直到你執行 `systemctl reset-failed emfor`。0.7.0 新增。 |
| `StateDirectory=emfor`、`StateDirectoryMode=0755` | 替 store 建立 `/var/lib/emfor`，擁有者是 `emfor`。 |
| `UMask=0027`、`ProtectProc=invisible`、`ProcSubset=pid` | 0.7.0 新增。不管這裡怎麼設，store 的檔案都會自己設定權限。 |
| `ProtectSystem=strict`、`NoNewPrivileges=yes`、`SystemCallFilter=@system-service` 等 | 加固。plugin 與 `exec` 檢查也受同樣的限制。 |
| `LimitNOFILE=65536` | 檢查、plugin 與 DNS 連線都會用到 file descriptor。 |
| `CapabilityBoundingSet=` | 不給任何 capability。DNS 要用 53 port 時，改成 `CapabilityBoundingSet=CAP_NET_BIND_SERVICE` 與 `AmbientCapabilities=CAP_NET_BIND_SERVICE`。 |

要注意的地方：

- 對 daemon 來說，除了 `/var/lib/emfor`，整個檔案系統都是唯讀的。`[store] path` 放在別的地方時，要在 drop-in 裡加上 `ReadWritePaths=`（`systemctl edit emfor`）。
- 不要用 `DynamicUser=`：它建立的 state 目錄，應用程式存取不到。
- unit 裡有一行註解掉的 `MemoryDenyWriteExecute=yes`。如果你的 plugin 都不會在執行期產生程式碼，就可以打開它；它在 systemd 252 上搭配 daemon 與 `dns` plugin 試過。
- log 會送進 journal：`journalctl -u emfor`。
- 你為舊版複製的範例 unit 不會自己更新：升級時記得拿它和新版比對（[第 15 章](15-upgrading.md)）。

## FreeBSD

`emfor` 已經在 FreeBSD ports tree 裡，是 `net-mgmt/emfor`（本文撰寫時是 0.7.1）。安裝套件，或自己建置 port：

```sh
# pkg install emfor
# make -C /usr/ports/net-mgmt/emfor install clean
```

[packages/freebsd/](../../packages/freebsd/) 留著這個 port 的一份副本，供參考，也供在新版進 ports tree 之前先試用。它用 `EMFOR_PREFIX=/usr/local` 建出兩個程式，安裝 rc.d script 與範例設定檔，並建立 `emfor` 使用者、空的 `/usr/local/etc/emfor/conf.d`，以及放 store 用的 `/var/db/emfor`。

```sh
# sysrc emfor_enable=YES
# service emfor start
# emfor status
```

| rc.conf 變數 | 預設值 | 意義 |
| --- | --- | --- |
| `emfor_enable` | `NO` | 開機時啟動。 |
| `emfor_config` | `/usr/local/etc/emfor/emfor.toml` | 設定檔。 |
| `emfor_user` | `emfor` | daemon 執行時的使用者。它要有 store 所在目錄的寫入權限。 |

- `service emfor configtest` 會檢查設定，`service emfor reload` 會先檢查再送 SIGHUP，`start` 遇到無效的設定會拒絕啟動。
- daemon 在 `daemon(8)` 底下執行；daemon 結束 2 秒後，`daemon(8)` 會再把它啟動。它的 log 以 tag `emfor` 送進 syslog。
- plugin 程式在 `/usr/local/bin/emfor-check`，不是 `/usr/bin`。
- 範例設定檔把 store 放在 `/var/db/emfor/state.mdb`，不是預設位置。主機上的每個應用程式都要設定 `EMFOR_STORE=/var/db/emfor/state.mdb`。

專案自己在 FreeBSD 15.1-RELEASE（arm64，在虛擬機器裡）上用 `make stage check-plist install` 建置並安裝過這個 port；沒在 13.x、14.x，或專案 CI 以外的 amd64 上跑過。

## Nix

repository 裡有一個 [flake](../../flake.nix)，含兩個套件和一個 NixOS module。0.7.1 沒有這個 flake，下一個發行版才會帶；在那之前請用 `main` 分支。emfor 不在 nixpkgs 裡。

```sh
$ nix build github:embark-systems/emfor#emfor
$ nix build github:embark-systems/emfor#libemfor
```

`emfor` 有 daemon 和 `emfor-check`；`libemfor` 是下面[client 函式庫](#client-函式庫)的 C 函式庫。兩個要用同一個 revision 建，和其他安裝方式一樣。

module [nix/module.nix](../../nix/module.nix) 會用 Nix 的設定寫出 `emfor.toml`，連到 `/etc/emfor/emfor.toml`，建立 `emfor` 使用者，並用和 deb、rpm 套件相同的加固 unit 執行 daemon：

```nix
{
  inputs.emfor.url = "github:embark-systems/emfor";

  outputs = { nixpkgs, emfor, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [
        emfor.nixosModules.default
        {
          services.emfor = {
            enable = true;
            settings = {
              site = "dc1";
              dns = { listen = [ "127.0.0.1:5300" ]; zone = "svc.emfor"; };
              services."api.svc.emfor" = {
                check = "tcp";
                backends = [ "10.0.0.1:8080" ];
              };
            };
          };
        }
      ];
    };
  };
}
```

- `settings` 填的是 `emfor.toml` 的鍵（[第 5 章](05-configuring.md)）。產生的檔案放在 Nix store，每個使用者都讀得到：不要把密鑰寫進去。要指定檔案的鍵（聯防的 secret、檢查的密碼檔）請指向 store 以外的檔案。
- unit 在啟動前和 reload 時都會跑 `emfor check-config`。`settings` 改了會 reload daemon，不會重新啟動。
- store 是 `/var/lib/emfor/state.mdb`。用預設的 `file_mode` 時，使用 emfor client 的應用程式，其使用者要在 `services.emfor.group` 指定的群組裡（預設是 `emfor`）。

這個 flake 在 aarch64 Linux 的 `nixos/nix` 容器裡（不開 Nix sandbox）建過：兩個套件都建得起來，daemon 的測試通過，`emfor check-config` 也接受 module 產生的檔案。專案還沒在 NixOS 機器上啟動過這個服務，也沒在 x86-64 Linux 或 macOS 上試過。

## client 函式庫

每個 client 都透過 `libemfor` 讀 store。`libemfor` 用 [cargo-c](https://github.com/lu-zero/cargo-c) 從 [clients/rust](../../clients/rust/README.md) 建出來，內含 emfor 自己的 LMDB build；系統的 liblmdb 讀不了 store。

```sh
$ cargo install cargo-c
$ cd clients/rust
# cargo cinstall --release --features capi --prefix /usr/local
```

這會裝好 `libemfor.so`（0.7.x 的 soname 是 `libemfor.so.0.7`）、`libemfor.a`、標頭檔 `emfor.h`，以及給 `pkg-config` 用的 `emfor.pc`。

| 語言 | 安裝 |
| --- | --- |
| Rust | 依賴同一個發行版的 `emfor-client` crate（`clients/rust`）。不需要 `libemfor`。 |
| C | 照上面的方式裝 `libemfor`；用 `cc app.c $(pkg-config --cflags --libs emfor)` 建置。 |
| Go | 先裝 `libemfor`，再 `go get github.com/embark-systems/emfor/clients/go@v0.8.0`。需要 cgo 與 `pkg-config emfor`。 |
| Python | 先裝 `libemfor`，再 `pip install clients/python`（Python 3.9 或更新；它用 `ctypes`）。函式庫不在標準目錄時，把 `EMFOR_LIBRARY` 設成函式庫的路徑。 |
| Perl | 先裝 `libemfor`，再到 `clients/perl` 執行：`perl Makefile.PL && make -f Makefile.xs && make -f Makefile.xs install`。 |
| PHP | 先裝 `libemfor`，再從 `clients/php` 的複本用 `phpize` 建出 extension（見[第 9 章](09-clients.md#php)），並啟用 `extension=emfor`。需要 PHP 8.1 或更新。 |

盡量用 shared library：它只匯出 `emfor_*` 符號，所以裡面的 LMDB 不會和同一個行程裡的其他 LMDB 衝突。

各個 client 怎麼用，見[第 9 章](09-clients.md)。
