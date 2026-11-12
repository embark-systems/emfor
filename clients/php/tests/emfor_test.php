<?php
// Tests for the emfor extension. The Makefile loads the built extension and
// writes clients/c/tests/fixture.json to EMFOR_STORE: api.svc.emfor,
// round-robin over the two backends below. Prints PASS/FAIL lines; exits 1
// on any failure. The fork check needs pcntl; the dba check runs when the dba
// lmdb handler exists, and EMFOR_REQUIRE_DBA=1 makes its absence a failure.

$backends = ['10.0.0.11:8080', '10.0.0.12:8080'];
$failed = 0;
function check(bool $ok, string $what): void {
    global $failed;
    echo ($ok ? "PASS " : "FAIL "), $what, "\n";
    if (!$ok) $failed++;
}
function thrown(callable $f): ?Throwable {
    try { $f(); } catch (Throwable $e) { return $e; }
    return null;
}

check(extension_loaded('emfor'), 'extension loaded');
check(getenv('EMFOR_STORE') !== false, 'EMFOR_STORE set');

$a = Emfor\lookup('api.svc.emfor');
$b = Emfor\lookup('API.svc.emfor.', '');
check(in_array($a, $backends, true), "address $a");
check(in_array($b, $backends, true) && $a !== $b, "next address $b alternates");
check(in_array(Emfor\lookup('api.svc.emfor', null), $backends, true), 'null key');

$e = thrown(fn() => Emfor\lookup('nope.svc.emfor'));
check($e instanceof Emfor\LookupException && $e instanceof RuntimeException, 'missing service throws LookupException');
check($e->errorCode === 'NOT_FOUND', 'errorCode NOT_FOUND');
check($e->getCode() === 3, 'getCode() is the C status');
check(str_starts_with($e->getMessage(), 'NOT_FOUND: ') && strlen($e->getMessage()) > 11, 'message');
check(thrown(function () use ($e) { $e->errorCode = 'X'; }) instanceof Error, 'errorCode is readonly');

foreach ([["api\0svc", null], ['api.svc.emfor', "a\0b"], ["\xff", null]] as [$n, $k]) {
    $x = thrown(fn() => Emfor\lookup($n, $k));
    check($x instanceof ValueError, 'ValueError for ' . bin2hex($n) . '/' . bin2hex((string)$k));
}

if (function_exists('pcntl_fork')) {
    $pid = pcntl_fork();
    if ($pid === 0) {
        $ok = in_array(Emfor\lookup('api.svc.emfor'), $backends, true);
        exit($ok ? 0 : 1);
    }
    pcntl_waitpid($pid, $status);
    check(pcntl_wifexited($status) && pcntl_wexitstatus($status) === 0, 'lookup in a forked child');
    check(in_array(Emfor\lookup('api.svc.emfor'), $backends, true), 'parent after fork');
} else {
    check(false, 'pcntl is loaded (needed for the fork check)');
}

if (function_exists('dba_handlers') && in_array('lmdb', dba_handlers(), true)) {
    // The system's LMDB (through dba) and libemfor's in one process.
    $path = sys_get_temp_dir() . '/emfor-php-dba-' . getmypid() . '.mdb';
    $dba = dba_open($path, 'c', 'lmdb');
    check($dba !== false, 'dba_open with the system LMDB');
    dba_replace('k', 'from-system-lmdb', $dba);
    check(in_array(Emfor\lookup('api.svc.emfor'), $backends, true), 'lookup next to dba lmdb');
    check(dba_fetch('k', $dba) === 'from-system-lmdb', 'dba store still readable');
    dba_close($dba);
    @unlink($path);
    @unlink("$path-lock");
} elseif (getenv('EMFOR_REQUIRE_DBA') === '1') {
    check(false, 'dba with the lmdb handler is loaded (EMFOR_REQUIRE_DBA=1)');
} else {
    echo "SKIP dba: no lmdb handler (set EMFOR_REQUIRE_DBA=1 to require it)\n";
}

echo $failed ? "FAIL $failed\n" : "PASS all\n";
exit($failed ? 1 : 0);
