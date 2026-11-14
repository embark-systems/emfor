## Context

client-lookup requirement「Sticky uses rendezvous hashing version 1」規定：`sticky` 帶非空 affinity key 時，對每個候選算 `MD5(key bytes ++ id bytes)` 前 4 bytes（big-endian u32），最大者勝出，相同時設定檔順序在前者勝出。實作在 `clients/rust/src/select.rs` 的 `rendezvous_hash`／`rendezvous_index`，`pick` 對 `(Balance::Sticky, Some(k))` 直接回傳，不看 `weight` 與慢啟動。C、Go、Python、Perl、PHP 都呼叫 libemfor（`clients/go/emfor.go`、`clients/python/emfor/__init__.py`、`clients/perl/Emfor.xs` 等），沒有自己的 MD5 實作。`clients/rust/src/lib.rs` 對外公開 `rendezvous_hash` 與 `rendezvous_pick`，`tests/conformance/v1/sticky-v1.json` 的向量只由 Rust runner 驗 hash。

## Goals / Non-Goals

**Goals:**

- 帶 key 的 `sticky` 依 `weight` 分配 key，比例為 w / 權重總和。
- 權重全為 1 時每個 key 的結果與現在完全相同。
- 只用整數與 MD5，跨平台、跨語言結果一致。

**Non-Goals:**

- 浮點加權 rendezvous、慢啟動對帶 key 的 `sticky` 生效、沒有 key 的 `sticky`、DNS 回答、新的 `balance` 名稱。

## Decisions

### 用虛擬節點做加權 rendezvous

權重 w 的候選，分數是 `h_0 … h_(w-1)` 的最大值：`h_0 = u32be(MD5(key ++ id)[0..4])`，`h_i = u32be(MD5(key ++ id ++ "#" ++ i 的十進位)[0..4])`（i ≥ 1）。分數最大的候選勝出；分數相同時設定檔順序在前者勝出。

所有虛擬節點的值獨立且均勻，所以某個候選擁有全體最大值的機率是 w / Σw。`h_0` 沿用現在的定義，所以權重 1 的候選分數不變；權重從 w 提高到 w' 只增加該候選的虛擬節點，只會讓 key 移到它身上。

替代方案：`score = -w / ln(h / 2^32)`，每個候選只算一次 MD5。但 `ln` 依賴平台數學函式庫，結果不保證 bit 一致；不同主機上的 libemfor 可能把同一個 key 送到不同 backend。不採用。

### 直接改 sticky，不新增 balance 名稱

`weight` 本來就被任何 `balance` 接受，在 `sticky` 底下卻沒有作用，容易誤會。直接讓 `sticky` 使用 `weight`，權重全為 1 的既有部署完全不受影響。設了非 1 權重的 sticky 服務會有部分 key 移動，在 CHANGELOG 說明。

### 公開函式加上權重版本

`clients/rust/src/lib.rs` 的 `rendezvous_pick(key, ids)` 保留（等同全部權重 1），另外新增 `rendezvous_pick_weighted(key, &[(id, weight)])`；`select.rs` 的 `rendezvous_index` 改為接受 `(id, weight)`。保留舊函式避免破壞已使用它的程式。

### 權重超過 100 時當成 100

daemon 設定只允許權重 1 到 100，但 client 從 store 讀到的 `weight` 是沒有檢查的 `u32`。每單位權重要算一次 MD5，壞掉或手改的 store 若寫入接近 `u32::MAX` 的權重，一次查詢要算約 40 億次 MD5（以每次 0.07 µs 估計約 5 分鐘）。因此 `rendezvous_score` 把超過 100 的權重當成 100、0 當成 1，每個候選最多算 100 次。daemon 寫出的權重都在 1 到 100，所以正常的 store 結果不變。

替代方案：把權重超過 100 的 record 當成格式錯誤。但其他 `balance`（例如 `weighted`）照常使用這些權重，只在 sticky 拒絕會讓同一份 store 在不同 `balance` 下有不同的錯誤行為。不採用。

### 先量測，再決定

每次查詢的 MD5 次數變成候選的權重總和（每台最多 100）。估計 10 台權重 100 的查詢為數十微秒，尚未量測。第一個 task 量測 10 台、權重各 1、10、100 的查詢時間；10×100 超過 100 µs 就停下回報，改議其他做法（例如快取每個 key 的結果）。

量測結果（2026-11-14，release build，`select.rs` 的 `measure_weighted_sticky`：10 個候選，每種權重 100000 個不同的 key，三次取中位數；MD5 狀態在 `key ++ id` 之後複製，每個虛擬節點只再算 `#i`）：

| 權重（10 台都相同） | Apple M4 Max（macOS） | 同一台機器上的 Debian 容器 |
| --- | --- | --- |
| 1 | 0.78 µs | 0.84 µs |
| 10 | 7.24 µs | 7.55 µs |
| 100 | 72.39 µs | 74.73 µs |

時間與權重總和成正比（每個虛擬節點約 0.07 µs）。10×100 低於 100 µs 門檻，繼續實作；較慢的 CPU 可能接近或超過門檻，手冊與 CHANGELOG 會寫出這個成本。

## Implementation Contract

**行為**

- `balance = "sticky"`、affinity key 非空：依「用虛擬節點做加權 rendezvous」選出候選。候選集合的決定（健康、備援層、`on_all_down`）不變。
- 權重全為 1：與 requirement「Sticky uses rendezvous hashing version 1」結果相同。
- 慢啟動不影響帶 key 的 `sticky`；沒有 key 時照 round-robin，也與現在相同。

**介面**

- `select::rendezvous_index(key, candidates: impl Iterator<Item = (&str, u32)>) -> usize`。
- `emfor_client::testing` 新增 `rendezvous_pick_weighted(key: &str, ids: &[(&str, u32)]) -> &str`（和既有的 `rendezvous_pick` 同一個模組）；`rendezvous_hash`、`rendezvous_pick` 不變。

**測試資料（以 Python hashlib 計算）**

| Key | Id | Hash (u32) |
| --- | --- | --- |
| `user-42` | `10.0.0.11:8080#1` | 101315482 |
| `user-42` | `10.0.0.11:8080#2` | 3080192789 |
| `user-42` | `10.0.0.13:8080#1` | 491054110 |
| `user-7` | `10.0.0.13:8080#1` | 2987525892 |

| Key | 權重（.11, .12, .13） | 結果 |
| --- | --- | --- |
| `user-42` | 1, 1, 1 | `10.0.0.12:8080` |
| `user-42` | 3, 1, 1 | `10.0.0.11:8080` |
| `user-42` | 1, 1, 100 | `10.0.0.13:8080` |
| `user-7` | 1, 1, 1 | `10.0.0.11:8080` |
| `user-7` | 1, 1, 100 | `10.0.0.13:8080` |
| `user-7` | 2, 2, 2 | `10.0.0.11:8080` |

`key-0` 到 `key-999` 在權重 1、1、2 時，`.13` 分到 499 個；從 1、1、1 改成 1、1、2 時有 151 個 key 移動，全部移到 `.13`。

**驗收**

- `select.rs` 單元測試：上述 hash 與選擇表、權重 1 與舊算法逐一相同（1000 個 key）、1、1、2 的分配與移動。
- `cases.json` 新增加權 sticky 案例，所有 wrapper 透過 `emfor-conformance` 驗證；既有 sticky 案例與 `sticky-v1.json` 不變且通過。
- 每個新測試在拿掉權重時失敗。

**範圍**

- 在範圍內：`clients/rust`、`tests/conformance/v1`、手冊第 3 與 5 章、CHANGELOG。
- 不在範圍內：daemon、store 格式、各語言 wrapper 的程式碼、`emfor-conformance` driver。

## Risks / Trade-offs

- [設了非 1 權重的 sticky 服務升級時部分 key 換 backend] → CHANGELOG 標為行為變更；cache 類服務會暫時 miss。
- [新舊 libemfor 並存期間同一 key 可能選到不同 backend] → 只影響設了非 1 權重的 sticky 服務；建議同一服務的 client 一起升級。
- [查詢成本隨權重總和增加] → 先量測；超過門檻就停下。
