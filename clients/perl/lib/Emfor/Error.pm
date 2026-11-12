package Emfor::Error;

# A failed Emfor::lookup: code is UNAVAILABLE, INCOMPATIBLE, NOT_FOUND or
# ALL_DOWN (the names shared by every emfor client); it stringifies as
# "CODE: message".

use strict;
use warnings;

use overload '""' => sub { "$_[0]{code}: $_[0]{message}" }, fallback => 1;

sub new {
    my ($class, %args) = @_;
    return bless {code => $args{code}, message => $args{message}}, $class;
}

sub code    { $_[0]{code} }
sub message { $_[0]{message} }

1;
