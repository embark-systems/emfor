<?php
// PHP lookup program for the shared conformance driver (tools/emfor-fixture,
// emfor-conformance): <name> <affinity-key or ""> <count>, printing
// "OK <address>" or "ERR <CODE> <message>" per lookup.
if ($argc !== 4) {
    fwrite(STDERR, "usage: conformance_lookup.php <name> <affinity-key> <count>\n");
    exit(2);
}
[, $name, $key, $count] = $argv;
for ($i = 0; $i < (int)$count; $i++) {
    try {
        $addr = Emfor\lookup($name, $key); // before printing: it may throw
        echo "OK $addr\n";
    } catch (Emfor\LookupException $e) {
        echo 'ERR ', $e->errorCode, ' ', substr($e->getMessage(), strlen($e->errorCode) + 2), "\n";
    }
}
