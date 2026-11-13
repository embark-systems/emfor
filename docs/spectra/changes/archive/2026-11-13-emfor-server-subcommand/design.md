## Context

`emfor` 的命令列由 crates/emfor-daemon/src/cli.rs 的 clap 結構 `Cli` 定義：全域 `--config`（預設為編譯時決定的 `DEFAULT_CONFIG_PATH`）、旗標 `--check-config`，以及唯一的子命令 `status`。`main` 在沒有子命令時建立 tokio runtime 並執行 `start`：`--check-config` 時只驗證設定，否則啟動 daemon。

因此不帶參數的 `emfor` 就是啟動 daemon。在已有 daemon 執行的主機上，這會得到 store writer lock 被佔用的錯誤；在沒有 daemon 的主機上，則會在前景真的啟動一個。使用者在 FreeBSD 上就遇到前者。

0.1.0 與 0.1.1 已發布，rc.d、systemd 範例與部署文件都以 `emfor --config ...` 啟動、以 `emfor --check-config` 檢查設定。

## Goals / Non-Goals

**Goals:**

- 不帶子命令的 `emfor` 只印用法，不碰設定檔、store 與外掛。
- 啟動 daemon 與檢查設定各有明確的子命令：`emfor server`、`emfor check-config`，與 `emfor status` 並列。
- 從 0.1.x 升級的人照到舊命令時，錯誤訊息直接說出新命令。
- repository 內所有範例、文件、測試與測試腳本都改用新命令。

**Non-Goals:**

- 不保留舊用法的相容模式（不帶子命令仍啟動 daemon、或 `--check-config` 仍能用）。
- 不改 daemon 本身的行為、結束碼、訊號處理、log 格式與設定檔格式。
- 不改 `emfor status` 的選項與輸出。
- 不新增其他子命令（例如 `emfor reload`、`emfor version`）。
- 不在本 change 內改版本號；0.2.0 依 RELEASING.md 在發版時處理。

## Decisions

### 子命令名稱用 server

用 `emfor server` 啟動 daemon。這是使用者提出的名稱，與 `vault server`、`consul agent` 這類「一個執行檔、多個子命令、其中一個是長駐服務」的工具一致。考慮過 `run`（容易被誤解為跑一次檢查）、`daemon`（daemon 並不自行 fork 到背景，名稱會讓人以為會）、`serve`（與 `server` 差別不大，選使用者提的）。

### check-config 改為子命令，舊旗標移除

`--check-config` 改為子命令 `emfor check-config`，與 `server`、`status` 同一層。留在 `emfor server --check-config` 也可行，但檢查設定不是「啟動 server 的一種模式」，作為獨立子命令較好理解，也讓 `emfor --help` 一眼看到三件事。舊旗標不保留別名：既然啟動命令本身已不相容，一次改完比留兩套用法清楚。

### 沒有子命令時印用法並以 2 結束

不帶子命令（含只給 `--config` 的情況）時印出用法到 stderr，以 2 結束，與其他用法錯誤同一個結束碼（clap 的慣例，也等於 `EXIT_CONFIG`）。用法列出 `server`、`check-config`、`status` 三個子命令。`emfor --help` 與 `emfor -h` 仍以 0 結束、印到 stdout，並照舊顯示內建的預設設定檔路徑。

### 舊旗標給出改用提示

`--check-config` 保留為隱藏的旗標，只用來在出現時印出 `--check-config was replaced by "emfor check-config"` 並以 2 結束；它不在 `--help` 中出現，也不執行任何檢查。沒有子命令但帶了 `--config` 時（0.1.x 的啟動方式），印出的用法第一個列出的就是 `server`。這樣 systemd 的 `ExecStartPre=emfor --check-config` 或 rc.d 舊腳本在升級後會以清楚的訊息失敗，而不是 clap 的 `unexpected argument`。

## Implementation Contract

**行為**

- `emfor`（無參數）：stderr 印用法（含 `server`、`check-config`、`status`），結束碼 2。不讀設定檔，不建立 store 檔或其 `-lock` 檔，不啟動外掛。
- `emfor --config <path>`（無子命令）：同上，結束碼 2。
- `emfor server [--config <path>]`：與 0.1.1 不帶子命令的 `emfor` 完全相同（啟動、設定錯誤結束碼 2、store 打不開或被鎖結束碼 1、SIGHUP 重載、SIGTERM/SIGINT 結束碼 0）。
- `emfor check-config [--config <path>]`：與 0.1.1 的 `emfor --check-config` 完全相同（成功印 `config ok: <n> services, <m> backends` 結束碼 0；失敗每行一個錯誤結束碼 2；啟動並停止外掛；不開 store）。
- `emfor status ...`：不變。
- `--config` 是全域選項：`emfor --config X server` 與 `emfor server --config X` 等價，`check-config`、`status` 同理。
- `emfor --check-config`（不論是否帶 `--config` 或子命令）：stderr 印 `--check-config was replaced by "emfor check-config"`，結束碼 2，不做任何檢查。
- `emfor --help`：stdout，結束碼 0，列出三個子命令與內建預設設定檔路徑。

**介面**

- crates/emfor-daemon/src/cli.rs：`Command` 新增 `Server` 與 `CheckConfig` 兩個沒有自身選項的變體；`Cli.check_config` 改為 `hide = true` 的旗標，只用於印提示。`Cli.command` 維持 `Option<Command>`，不用 clap 的 `subcommand_required`，好讓 `--check-config` 的提示優先於「缺少子命令」。`main` 依序處理：`check_config` 為真時印提示並結束 2；`command` 為 `None` 時把 `Cli::command()` 的說明寫到 stderr 並結束 2；`Status` 照舊；`Server` 與 `CheckConfig` 建立 runtime 後呼叫現有的啟動與檢查流程。

**驗收**

- crates/emfor-daemon/tests/daemon.rs 新增測試：無參數與只帶 `--config` 時結束碼 2、stderr 含 `server`、`check-config`、`status`，且暫存目錄中沒有產生 store 檔；`--check-config` 時結束碼 2 且 stderr 含 `emfor check-config`。
- 既有整合測試全部改用 `server` / `check-config` 後通過：crates/emfor-daemon/tests/ 中的 daemon、dns、metrics、plugins。
- `help_shows_the_built_in_config_path` 仍通過。
- `git grep -n -- "--check-config"` 在 docs/spectra/changes 與 CHANGELOG 以外只剩 cli.rs 的隱藏旗標與其測試；以 `emfor --config` 啟動 daemon 的寫法在範例、文件、測試腳本中不再出現。
- 本機檢查：macOS 與 Debian 映像的 fmt、clippy -D warnings、cargo test --workspace --all-features；tests/resolver 的 dnsmasq、unbound 容器測試與 tests/plugins/run.sh 通過（它們都以腳本啟動 daemon）。

**範圍**

- 範圍內：cli.rs、上述整合測試、examples/rc.d/emfor、examples/systemd/emfor.service、examples/emfor.toml 開頭說明、examples/freebsd/Makefile 註解與 README、README.md、README.zh-TW.md、docs/deploy/daemon.md、docs/deploy/plugins.md、docs/plugins/protocol-v1.md、tests/resolver/、tests/plugins/、tests/spike/run.sh、CHANGELOG.md 的 Unreleased 段。
- 範圍外：版本號、client、emfor-check、store 格式、已 archive 的 change 文件。

## Risks / Trade-offs

- [已部署 0.1.x 的主機升級後無法啟動] → CHANGELOG 開頭寫明要改 `emfor server` 與 `emfor check-config`；舊旗標與無子命令都以 2 結束並印出新命令，systemd 與 rc.d 會在啟動時就失敗，而不是悄悄改變行為。
- [隱藏的 `--check-config` 旗標是多留的一點程式碼] → 只印一行提示，可在之後的版本移除；收益是升級時的錯誤訊息直接可用。
- [tests/resolver/freebsd.sh 沒有本機 FreeBSD 環境可跑] → 照樣更新，與 rc.d 範例一起留給手動的 FreeBSD 驗證；使用者已有 FreeBSD 主機可實測。
