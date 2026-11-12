# Builds the Emfor XS module against libemfor, found with pkg-config:
#   perl Makefile.PL && make && make test
use strict;
use warnings;
use ExtUtils::MakeMaker;

sub pc {
    my $out = `pkg-config @_ emfor`;
    die "pkg-config @_ emfor failed: install libemfor (clients/rust, cargo cinstall --features capi) first\n"
        if $?;
    chomp $out;
    return $out;
}

my $libdir = pc('--variable=libdir');
WriteMakefile(
    NAME           => 'Emfor',
    VERSION_FROM   => 'lib/Emfor.pm',
    ABSTRACT       => 'Look up a healthy backend in the local emfor state store',
    LICENSE        => 'freebsd',  # CPAN::Meta's name for BSD 2-Clause
    INC            => pc('--cflags'),
    LIBS           => [ pc('--libs') ],
    # Find libemfor where pkg-config found it, without LD_LIBRARY_PATH.
    dynamic_lib    => { OTHERLDFLAGS => "-Wl,-rpath,$libdir" },
    # clients/perl/Makefile drives the build; keep MakeMaker's apart.
    FIRST_MAKEFILE => 'Makefile.xs',
    clean          => { FILES => 'Makefile.xs.old' },
);
