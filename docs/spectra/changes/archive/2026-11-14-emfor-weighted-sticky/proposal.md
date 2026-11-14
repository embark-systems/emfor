## Why

`balance = "sticky"` 帶 affinity key 時，client 以 rendezvous hashing 選 backend：對每個候選算 `MD5(key + id)` 取前 4 bytes，最大的勝出。這個算法完全不看 `weight`，所以規格不同的機器在 sticky 服務裡分到一樣多的 key。設定檔接受 `weight`，但在 `sticky` 底下它其實沒有作用，使用者容易以為有效。

## What Changes

- `sticky` 帶非空 affinity key 時依 `weight` 分配 key：權重 w 的候選，分數是 `h_0` 到 `h_(w-1)` 的最大值，其中 `h_0 = MD5(key + id)`（與現在相同），`h_i = MD5(key + id + "#" + i)`（i 從 1 起，十進位）；分數最大者勝出，相同時設定檔順序在前者勝出。每個 backend 分到的 key 比例是 w / 權重總和。
- 權重全為 1 的服務，每個 key 選到的 backend 與現在完全相同。調高某台的權重時，只有改選這一台的 key 會移動。
- 只有 Rust 實作（`clients/rust/src/select.rs`）需要修改；C、Go、Python、Perl、PHP 都經由 libemfor，自動跟上。
- 第一個 task 先量測查詢時間（10 台 backend，權重各 1、10、100），超過門檻就停下回報。
- **行為變更**：已經在 `sticky` 服務設定非 1 權重的部署，升級後有一部分 key 會換 backend；新舊 client 並存的升級期間，兩邊對這些 key 的選擇不同。CHANGELOG 會標明。
- 手冊第 3 章（`weight` 用於 `weighted` 與 `sticky`）、第 5 章 sticky 一節（英文與中文）、conformance 案例與 CHANGELOG 跟著更新。

## Non-Goals

- 加權 rendezvous 的浮點公式 `-w / ln(h)`：`ln` 在不同平台的數學函式庫結果不保證 bit 一致，會破壞「同一個 key 在每台主機都選同一台」。
- 讓帶 key 的 `sticky` 受慢啟動影響：維持現狀，不受影響。
- 沒有 key 的 `sticky`（照 round-robin）與 DNS 回答（沒有 affinity key）：不變。
- 新增 `balance` 名稱：直接讓 `sticky` 使用 `weight`，不另立 `weighted-sticky`。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `client-lookup`: 以 requirement「Sticky uses weighted rendezvous hashing」取代「Sticky uses rendezvous hashing version 1」。

## Impact

- 受影響的 spec：`client-lookup`
- 受影響的程式碼：
  - clients/rust/src/select.rs
  - clients/rust/src/lib.rs
  - clients/rust/tests/conformance.rs
  - tests/conformance/v1/cases.json
  - tests/conformance/v1/README.md
  - docs/manual/03-configuration.md
  - docs/manual.zh_TW/03-configuration.md
  - docs/manual/05-backend-selection.md
  - docs/manual.zh_TW/05-backend-selection.md
  - CHANGELOG.md
- daemon、store 格式、client API 與 `tests/conformance/v1/sticky-v1.json` 不變。
