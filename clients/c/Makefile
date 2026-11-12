# C tests for libemfor. `make test` installs libemfor from clients/rust into
# build/prefix with cargo-c, builds the tests with pkg-config, writes a store
# with emfor-fixture and runs them. Needs cargo, cargo-c, pkg-config and cc.

ROOT    := $(abspath ../..)
BUILD   := $(abspath build)
PREFIX  := $(BUILD)/prefix
STORE   := $(BUILD)/state.mdb
CFLAGS  ?= -O2
CFLAGS  += -std=c11 -D_POSIX_C_SOURCE=200809L -Wall -Wextra -Werror
PC      := PKG_CONFIG_PATH=$(PREFIX)/lib/pkgconfig pkg-config
TESTS   := test_lookup test_fork test_threads
RUN     := EMFOR_STORE=$(STORE) EMFOR_MAX_STALENESS=60s
ifeq ($(shell uname),Darwin)
SHLIB   := $(PREFIX)/lib/libemfor.dylib
else
SHLIB   := $(PREFIX)/lib/libemfor.so
endif

.PHONY: all test install clean
all: $(addprefix $(BUILD)/,$(TESTS)) $(BUILD)/example

# Always reinstall, so the tests use the current Rust sources.
install:
	cd $(ROOT)/clients/rust && cargo cinstall --release --features capi \
	    --prefix $(PREFIX) --libdir $(PREFIX)/lib >/dev/null

$(SHLIB): install

$(BUILD)/%: tests/%.c tests/common.h $(SHLIB)
	$(CC) $(CFLAGS) $$($(PC) --cflags emfor) $< -o $@ \
	    $$($(PC) --libs emfor) -Wl,-rpath,$(PREFIX)/lib -lpthread

$(BUILD)/example: example.c $(SHLIB)
	$(CC) $(CFLAGS) $$($(PC) --cflags emfor) $< -o $@ \
	    $$($(PC) --libs emfor) -Wl,-rpath,$(PREFIX)/lib

# A fresh store each run: its heartbeat is the time it was written.
test: all
	cd $(ROOT) && cargo run -q -p emfor-fixture -- \
	    $(abspath tests/fixture.json) 0 $(STORE)
	sh tests/check_symbols.sh $(SHLIB)
	for t in $(TESTS); do $(RUN) $(BUILD)/$$t || exit 1; done
	$(RUN) $(BUILD)/example api.svc.emfor user-42
	@echo "PASS all C tests"

clean:
	rm -rf build
