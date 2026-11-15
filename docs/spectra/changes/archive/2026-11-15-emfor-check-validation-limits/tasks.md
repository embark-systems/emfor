## 1. 確認 database-checks 的 scenario 都有測試

- [x] 1.1 確認 "Check options validation" 的 pgsql NUL 密碼規則與範例列，和 `crates/emfor-check/src/options.rs` 的 `check_nul_free_password_file` 一致（錯誤訊息逐字為 `password must not contain NUL`），並由 `options::tests::option_validation_table` 的 pgsql `nul.pass` 列涵蓋。驗證：在 emfor-clients-test 映像跑 `cargo test -p emfor-check --lib option_validation_table` 通過。
- [x] 1.2 確認 "Passwords from password_file only" 的新 scenario：pgsql 檢查讀到含 NUL 的密碼時，在連線前回報 `error`，reason 為 `password_file: password must not contain NUL`，由 `pgsql::tests::password_with_nul_is_error` 涵蓋。驗證：`cargo test -p emfor-check --lib password_with_nul_is_error` 通過。

## 2. 確認 network-checks 的 scenario 都有測試

- [x] 2.1 確認 "AMQP check options" 的 Start-Ok 4096 bytes 規則：驗證時擋下多一個 byte 的密碼、接受剛好放得下的密碼，訊息含 `at most <n> bytes with this user`，由 `amqp::tests::amqp_login_fits_the_first_frame` 涵蓋；剛好放得下的密碼能登入 RabbitMQ，由 live 測試 `amqp_live` 涵蓋。驗證：`cargo test -p emfor-check --lib amqp_login_fits_the_first_frame` 通過，並引用 47cf22c 記錄的 `tests/plugins/run.sh` 容器中 `amqp_live` 通過的結果。
- [x] 2.2 確認 "AMQP check options" 的檢查時規則：密碼檔事後變長時，檢查在連線前回報 `error`，reason 以 `password_file: password too long` 開頭，由 `amqp::tests::too_long_password_is_error` 涵蓋。驗證：`cargo test -p emfor-check --lib too_long_password_is_error` 通過。
- [x] 2.3 確認 "Diameter check options" 的 Message Length 規則：以 IPv6 Host-IP-Address 計算，剛好放得下的 `supported_vendor_ids` 數量沒有錯誤，多一筆時在 `supported_vendor_ids` 回報 `too many entries: ...`，由 `diameter::tests::capabilities_exchange_request_fits_the_length_field` 涵蓋。驗證：`cargo test -p emfor-check --lib capabilities_exchange_request_fits_the_length_field` 通過。

## 3. 檢查 change 本身

- [x] 3.1 [after: 1.1, 1.2, 2.1, 2.2, 2.3] 確認 delta specs 只改這四條需求，其他文字與歸檔前的 specs 相同。驗證：`spectra validate emfor-check-validation-limits` 通過，且 `spectra archive emfor-check-validation-limits --preview --json` 只列出 database-checks 與 network-checks 各兩條 modified requirement。
