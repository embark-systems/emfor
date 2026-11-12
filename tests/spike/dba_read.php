<?php
// Spike: read the emfor store read-only with PHP's dba lmdb handler.
// Usage: php dba_read.php <store path>
// Exit 0 when dba lists "lmdb", the open succeeds and
// meta/schema_version is "1".
echo "php: ", PHP_VERSION, "\n";
$handlers = dba_handlers(true);
echo "dba handlers: ", implode(", ", array_map(
    fn($k, $v) => "$k ($v)", array_keys($handlers), $handlers)), "\n";
if (!isset($handlers["lmdb"])) {
    echo "FAIL dba has no lmdb handler\n";
    exit(1);
}
$db = @dba_open($argv[1], "r", "lmdb");
if ($db === false) {
    $e = error_get_last();
    echo "FAIL dba_open: ", $e ? $e["message"] : "unknown error", "\n";
    exit(1);
}
$v = dba_fetch("meta/schema_version", $db);
echo "schema_version: ", var_export($v, true), "\n";
echo "heartbeat: ", var_export(dba_fetch("meta/heartbeat", $db), true), "\n";
dba_close($db);
if ($v !== "1") {
    echo "FAIL schema_version is not 1\n";
    exit(1);
}
echo "PASS PHP\n";
