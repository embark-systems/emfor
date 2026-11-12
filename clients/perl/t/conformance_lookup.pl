# Perl lookup program for the shared conformance driver (tools/emfor-fixture,
# emfor-conformance): <name> <affinity-key or ""> <count>, printing
# "OK <address>" or "ERR <CODE> <message>" per lookup. Run with -Mblib.
use strict;
use warnings;
use Encode qw(decode);
use Emfor;

die "usage: conformance_lookup.pl <name> <affinity-key> <count>\n" unless @ARGV == 3;
# Emfor::lookup takes character strings; command-line arguments are bytes.
my ($name, $key, $count) = map { decode('UTF-8', $_) } @ARGV;
binmode STDOUT, ':encoding(UTF-8)';
for (1 .. $count) {
    my $addr = eval { Emfor::lookup($name, $key) };
    if (defined $addr) {
        print "OK $addr\n";
    } elsif (ref $@) {
        printf "ERR %s %s\n", $@->code, $@->message;
    } else {
        die $@;
    }
}
