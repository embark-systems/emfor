<?php
// Spike 2: read the emfor store through the probe library with FFI, in a
// process that has also opened a separate store with dba and the system's
// LMDB (when this PHP has the dba "lmdb" handler).
// Usage: php probe_read.php <shared library> <store path> <scratch dir>
// Exit 0 when the probe reads a positive heartbeat and, if dba lmdb exists,
// the dba store stays readable before and after.
[$_, $lib, $store, $scratch] = $argv;
echo "php: ", PHP_VERSION, "\n";
$dba = null;
if (in_array("lmdb", dba_handlers(), true)) {
    $dba = dba_open("$scratch/dba.mdb", "c", "lmdb");
    if ($dba === false) { echo "FAIL dba_open (system LMDB)\n"; exit(1); }
    dba_replace("k", "from-system-lmdb", $dba);
    echo "dba (system LMDB) before: ", dba_fetch("k", $dba), "\n";
} else {
    echo "note: no dba lmdb handler, same-process check skipped\n";
}
$ffi = FFI::cdef("int64_t probe_heartbeat(const char *path);", $lib);
$hb = $ffi->probe_heartbeat($store);
echo "heartbeat: $hb\n";
if ($hb <= 0) { echo "FAIL probe_heartbeat returned $hb\n"; exit(1); }
if ($dba !== null) {
    $v = dba_fetch("k", $dba);
    echo "dba (system LMDB) after: $v\n";
    if ($v !== "from-system-lmdb") { echo "FAIL dba store unreadable after probe\n"; exit(1); }
    dba_close($dba);
    echo "PASS PHP (same process as dba lmdb)\n";
} else {
    echo "PASS PHP\n";
}
