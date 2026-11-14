# The Makefile writes clients/c/tests/fixture.json to EMFOR_STORE:
# api.svc.emfor, round-robin over the two backends below, and
# sticky.svc.emfor, sticky over the same two.
use strict;
use warnings;
use Test::More;

use Emfor;

my %backend = map { $_ => 1 } ('10.0.0.11:8080', '10.0.0.12:8080');

like(eval { Emfor::lookup("api\0svc") } // $@, qr/^Emfor::lookup: invalid argument/, 'NUL in name');
like(eval { Emfor::lookup('api.svc.emfor', "a\0b") } // $@, qr/^Emfor::lookup: invalid argument/, 'NUL in key');
ok(!ref $@, 'invalid argument is a plain string');

SKIP: {
    skip 'EMFOR_STORE not set; run make test', 15 unless $ENV{EMFOR_STORE};

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

    # "café" as a character string picks what every client picks for the
    # UTF-8 key "café" (pinned in clients/rust/src/select.rs,
    # non_ascii_key_pick), however Perl stores it: Latin-1 without the UTF8
    # flag (a "café" literal under `use utf8` is stored this way), upgraded,
    # or decoded from UTF-8 bytes.
    my $latin1 = "caf\x{e9}";
    my $upgraded = $latin1;
    utf8::upgrade($upgraded);
    require Encode;
    my $decoded = Encode::decode('UTF-8', "caf\xc3\xa9");
    ok(!utf8::is_utf8($latin1) && utf8::is_utf8($upgraded), 'both string forms tested');
    is(Emfor::lookup('sticky.svc.emfor', $latin1), '10.0.0.12:8080', 'Latin-1 key');
    is(Emfor::lookup('sticky.svc.emfor', $upgraded), '10.0.0.12:8080', 'upgraded key');
    is(Emfor::lookup('sticky.svc.emfor', $decoded), '10.0.0.12:8080', 'decoded key');
    # Undecoded UTF-8 bytes are the four characters "cafÃ©" (README).
    is(Emfor::lookup('sticky.svc.emfor', "caf\xc3\xa9"), '10.0.0.11:8080', 'bytes are characters');

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
