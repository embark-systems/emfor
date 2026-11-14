# C tests for libemfor. `make test` installs libemfor (clients/libemfor.mk),
# builds the tests with pkg-config, writes a store with emfor-fixture, runs
# them, and runs the conformance driver against tests/conformance_lookup.c.
# Needs cargo, cargo-c, pkg-config and cc.

include ../libemfor.mk

BUILD   := $(CLIENTS_BUILD)/c
STORE   := $(BUILD)/state.mdb
CFLAGS  ?= -O2
CFLAGS  += -std=c11 -D_POSIX_C_SOURCE=200809L -Wall -Wextra -Werror
TESTS   := test_lookup test_fork test_threads
RUN     := EMFOR_STORE=$(STORE) EMFOR_MAX_STALENESS=60s

.PHONY: all test clean FORCE
all: $(addprefix $(BUILD)/,$(TESTS) conformance_lookup) $(BUILD)/example

$(BUILD):
	mkdir -p $@

# Always rebuilt (FORCE): they are small, and clients/build may hold binaries
# for another platform (for example from a container sharing the checkout).
$(BUILD)/%: tests/%.c tests/common.h $(SHLIB) FORCE | $(BUILD)
	$(CC) $(CFLAGS) $$($(PC) --cflags emfor) $< -o $@ \
	    $$($(PC) --libs emfor) $(RPATH) -lpthread

$(BUILD)/example: example.c $(SHLIB) FORCE | $(BUILD)
	$(CC) $(CFLAGS) $$($(PC) --cflags emfor) $< -o $@ \
	    $$($(PC) --libs emfor) $(RPATH)

# A fresh store each run: its heartbeat is the time it was written.
test: all
	$(EMFOR_FIXTURE) $(abspath tests/fixture.json) 0 $(STORE)
	sh tests/check_symbols.sh $(SHLIB)
	sh tests/check_header.sh $(PREFIX)/include/emfor/emfor.h
	for t in $(TESTS); do $(RUN) $(BUILD)/$$t || exit 1; done
	$(RUN) $(BUILD)/example api.svc.emfor user-42
	$(EMFOR_CONFORMANCE) -- $(BUILD)/conformance_lookup
	@echo "PASS all C tests"

FORCE:

clean:
	rm -rf $(BUILD)
