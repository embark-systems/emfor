# Perl wrapper tests: `make test` installs libemfor (clients/libemfor.mk),
# builds the XS module with Makefile.PL (into Makefile.xs and blib/), runs
# t/*.t against a store written by emfor-fixture, and runs the conformance
# driver. Needs Perl with its headers and a C compiler.

include ../libemfor.mk

PERL  ?= perl
BUILD := $(CLIENTS_BUILD)/perl
STORE := $(BUILD)/state.mdb

.PHONY: test clean
test: libemfor
	mkdir -p $(BUILD)
	$(PERL) Makefile.PL
	$(MAKE) -f Makefile.xs
	$(EMFOR_FIXTURE) $(EMFOR_ROOT)/clients/c/tests/fixture.json 0 $(STORE)
	EMFOR_STORE=$(STORE) EMFOR_MAX_STALENESS=60s $(MAKE) -f Makefile.xs test
	$(EMFOR_CONFORMANCE) -- $(PERL) -I$(CURDIR)/blib/lib -I$(CURDIR)/blib/arch \
	    $(CURDIR)/t/conformance_lookup.pl
	@echo "PASS all Perl tests"

clean:
	-[ -f Makefile.xs ] && $(MAKE) -f Makefile.xs realclean
	rm -rf $(BUILD)
