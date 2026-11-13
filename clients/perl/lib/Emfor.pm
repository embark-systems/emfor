package Emfor;

# Look up a healthy backend for a service in the local emfor state store,
# through libemfor (the C library built from the emfor Rust client).
#
#   use Emfor;
#   my $addr = eval { Emfor::lookup('api.svc.emfor', 'user-42') };
#   if (my $err = $@) { warn $err->code eq 'ALL_DOWN' ? ... }
#
# The store path and staleness come only from EMFOR_STORE and
# EMFOR_MAX_STALENESS, read on the first lookup of a process. libemfor must
# come from the same emfor release as the daemon.

use strict;
use warnings;

use Emfor::Error;

our $VERSION = '0.2.0';

require XSLoader;
XSLoader::load('Emfor', $VERSION);

my %CODE = (1 => 'UNAVAILABLE', 2 => 'INCOMPATIBLE', 3 => 'NOT_FOUND', 4 => 'ALL_DOWN');

# lookup($name, $affinity_key) returns "ip:port" ("[ip]:port" for IPv6).
# $name and $affinity_key are character strings (decode bytes first, for
# example with Encode::decode('UTF-8', ...)); they are passed to libemfor as
# UTF-8. $affinity_key matters only for sticky services; undef or "" is no key.
# Dies with an Emfor::Error on failure, or with a string starting
# "Emfor::lookup: invalid argument" for an argument libemfor cannot take.
sub lookup {
    my ($name, $key) = @_;
    my ($status, $value) = _lookup($name, $key);
    return $value if $status == 0;
    die "Emfor::lookup: invalid argument: $value\n" if $status == 5;
    die Emfor::Error->new(code => $CODE{$status} // 'UNKNOWN', message => $value);
}

1;
