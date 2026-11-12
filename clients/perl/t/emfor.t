# The Makefile writes clients/c/tests/fixture.json to EMFOR_STORE:
# api.svc.emfor, round-robin over the two backends below.
use strict;
use warnings;
use Test::More;

use Emfor;

my %backend = map { $_ => 1 } ('10.0.0.11:8080', '10.0.0.12:8080');

like(eval { Emfor::lookup("api\0svc") } // $@, qr/^Emfor::lookup: invalid argument/, 'NUL in name');
like(eval { Emfor::lookup('api.svc.emfor', "a\0b") } // $@, qr/^Emfor::lookup: invalid argument/, 'NUL in key');
ok(!ref $@, 'invalid argument is a plain string');

SKIP: {
    skip 'EMFOR_STORE not set; run make test', 10 unless $ENV{EMFOR_STORE};

    my $a = Emfor::lookup('api.svc.emfor');
    my $b = Emfor::lookup('API.svc.emfor.', '');
    ok($backend{$a}, "address $a");
    ok($backend{$b} && $a ne $b, "next address $b alternates");

    ok(!defined eval { Emfor::lookup('nope.svc.emfor'); 1 }, 'missing service dies');
    my $err = $@;
    isa_ok($err, 'Emfor::Error');
    is($err->code, 'NOT_FOUND', 'code');
    ok(length $err->message, 'message');
    is("$err", 'NOT_FOUND: ' . $err->message, 'stringifies');

    # Only checks that a wide-character key is accepted. Its UTF-8 encoding is
    # checked by the conformance driver's sticky pick with key "使用者-1".
    is(Emfor::lookup('api.svc.emfor', "\x{4f7f}") && 1, 1, 'wide-character key accepted');

    my $pid = fork() // die "fork: $!";
    if ($pid == 0) {
        my $ok = eval { $backend{ Emfor::lookup('api.svc.emfor') } };
        POSIX::_exit($ok ? 0 : 1);
    }
    waitpid($pid, 0);
    is($? >> 8, 0, 'lookup in a forked child');
    ok($backend{ Emfor::lookup('api.svc.emfor') }, 'parent after fork');
}

use POSIX ();
done_testing();
