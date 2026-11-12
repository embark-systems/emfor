# emfor

[English](README.md) | 繁體中文

emfor 是在每台應用程式主機上執行的健康檢查 daemon。它檢查各服務的後端（TCP、HTTP、腳本，以及 redis、MySQL、PostgreSQL、LDAP 的外掛），把健康狀態寫進本機的狀態儲存（state store）。應用程式透過小型 client 函式庫取得一個健康的後端；答案來自本機檔案，查詢不經過網路，其他主機故障時也照常運作。

```
              經網路檢查
  後端 <------------------------------ emfor daemon
                                           |  寫入
                                           v
                                   本機 LMDB 狀態儲存
                                           |  讀取
                     +---------------------+---------------------+
                     |                     |                     |
          client 函式庫（Rust、C、    127.0.0.1 上的 DNS     emfor status
          Go、Python、Perl、PHP）    （給沒有 client 的程式）
```

- **健康檢查**：`tcp`（可選 TLS、送出與比對回應）、`http`/`https`、執行 Nagios 風格腳本的 `exec`，以及常駐的外掛。隨附的 `emfor-check` 提供 redis、MySQL/MariaDB、PostgreSQL 與 LDAP 外掛。後端連續通過 `rise` 次轉為 healthy，連續失敗 `fall` 次轉為 unhealthy。
- **挑選後端**：每個服務以 `balance` 指定 `round-robin`、`sticky`（依 affinity key）、`failover`、`weighted`、`locality`（同站點優先）或 `fastest`。`on_all_down` 決定沒有健康後端時怎麼處理。
- **Client**：只有一份 Rust 實作，同時編成 C 函式庫 `libemfor`，Go、Python、Perl、PHP 的 client 都包裝它。每種語言都回傳 `ip:port`，錯誤代碼也相同。
- **DNS**：可選的 DNS 伺服器，給沒有 client 的程式用；只回答自己的網域（預設 `svc.emfor`），放在主機的 resolver 後面。
- **維運**：`emfor status` 顯示 daemon 寫出的狀態，`/metrics` 提供 Prometheus 指標，`SIGHUP` 在設定完全正確時重新載入。

## 快速上手

建置 daemon（Rust 2024 edition）：

```sh
cargo build --release --bin emfor
```

寫一份只有一個服務的設定。這份把 store 放在 `/tmp`，不需要 root 也能跑：

```sh
mkdir -p /tmp/emfor
cat > /tmp/emfor/emfor.toml <<'EOF'
[store]
path = "/tmp/emfor/state.mdb"

[dns]

[metrics]

[services."web.svc.emfor"]
check = "http"
backends = ["127.0.0.1:8080"]
EOF
```

準備一個後端、檢查設定，然後啟動 daemon：

```sh
python3 -m http.server 8080 --bind 127.0.0.1 &
target/release/emfor --check-config --config /tmp/emfor/emfor.toml
target/release/emfor --config /tmp/emfor/emfor.toml
```

在另一個終端機查看結果：

```console
$ target/release/emfor status --config /tmp/emfor/emfor.toml
store: /tmp/emfor/state.mdb
daemon: running, heartbeat 0.0s ago, up 4.0s, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy  1.6s ago  4.5ms

$ dig +short @127.0.0.1 -p 5300 web.svc.emfor
127.0.0.1

$ curl -s 127.0.0.1:5301/metrics | grep 'state="healthy"'
emfor_backend_state{service="web.svc.emfor",backend="127.0.0.1:8080",state="healthy"} 1
```

把 8080 上的伺服器停掉：下一次檢查就會顯示失敗原因，連續失敗三次後（預設 `fall = 3`、每次間隔 2 秒）後端轉為 `unhealthy`。所有設定選項與說明見 [examples/emfor.toml](examples/emfor.toml)。

## 查詢後端

```rust
match emfor_client::lookup("web.svc.emfor", None) {
    Ok(addr) => println!("connect to {addr}"), // "127.0.0.1:8080"
    Err(e) => eprintln!("{e}"),                // 例如 "ALL_DOWN: ..."
}
```

其他語言的 client 用法相同；安裝方式與錯誤代碼見各自的 README（英文）：

| 語言 | Client |
| --- | --- |
| Rust | [clients/rust](clients/rust/README.md) |
| C（`libemfor`） | [clients/c](clients/c/README.md) |
| Go | [clients/go](clients/go/README.md) |
| Python | [clients/python](clients/python/README.md) |
| Perl | [clients/perl](clients/perl/README.md) |
| PHP | [clients/php](clients/php/README.md) |

C 函式庫以及所有建立在它之上的 client，必須和 daemon 來自同一個 emfor 版本：兩者用同一份 LMDB 建置，系統的 liblmdb 讀不了這個 store。

## 文件

以下文件都是英文。

| 主題 | 文件 |
| --- | --- |
| 安裝、`emfor status`、systemd、指標與告警 | [docs/deploy/daemon.md](docs/deploy/daemon.md) |
| 從 dnsmasq、unbound、systemd-resolved 或 macOS 轉發 DNS 網域 | [docs/deploy/dns-resolver.md](docs/deploy/dns-resolver.md) |
| 用 `emfor-check` 檢查 redis、MySQL、PostgreSQL 與 LDAP | [docs/deploy/plugins.md](docs/deploy/plugins.md) |
| 撰寫檢查外掛 | [docs/plugins/protocol-v1.md](docs/plugins/protocol-v1.md) |
| 所有設定選項 | [examples/emfor.toml](examples/emfor.toml) |
| systemd 單元檔、FreeBSD rc.d 腳本、Prometheus 告警規則 | [examples/](examples/) |
| 需求規格（spec） | [docs/spectra/specs/](docs/spectra/specs/) |

## 支援平台

正式部署平台是 Linux 與 FreeBSD；macOS 只用於開發。FreeBSD 尚未在本機環境測試過，由選用的 CI job 涵蓋。

## 目錄結構

| 路徑 | 內容 |
| --- | --- |
| `crates/emfor-daemon` | daemon 與 `emfor` 指令 |
| `crates/emfor-format` | 狀態儲存的鍵名與記錄格式（版本 1） |
| `crates/emfor-check` | `emfor-check` 外掛程式 |
| `crates/emfor-plugin` | 用 Rust 寫外掛 |
| `crates/emfor-tls` | `tcp` 檢查與 `emfor-check` 共用的 TLS |
| `clients/` | 各語言的 client 函式庫 |
| `tests/` | 跨語言相容性案例、外掛、resolver 與容器測試 |
| `tools/emfor-fixture` | 把相容性案例寫成 store 檔案 |

## 開發

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
make -C clients test
```

變更以 [Spectra](docs/spectra/) 的規格與變更提案規劃：需求在 `docs/spectra/specs/`，完成的變更在 `docs/spectra/changes/archive/`。

## 授權

BSD 2-Clause，見 [LICENSE](LICENSE)。
