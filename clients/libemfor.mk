# Shared by clients/<lang>/Makefile: installs libemfor from clients/rust into
# clients/build/prefix with cargo-c and defines what the wrappers' builds and
# tests need. Include it, then depend on `libemfor`.
#
#   include ../libemfor.mk
#   test: libemfor
#   	$(EMFOR_CONFORMANCE) -- <lookup program...>
#
# Needs cargo, cargo-c and pkg-config.

EMFOR_ROOT    := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
CLIENTS_BUILD := $(EMFOR_ROOT)/clients/build
PREFIX        := $(CLIENTS_BUILD)/prefix
LIBDIR        := $(PREFIX)/lib

ifeq ($(shell uname),Darwin)
SHLIB         := $(LIBDIR)/libemfor.dylib
else
SHLIB         := $(LIBDIR)/libemfor.so
endif

# Builds find libemfor through pkg-config; binaries find it through an rpath,
# so tests need no LD_LIBRARY_PATH. Python loads it from EMFOR_LIBRARY.
export PKG_CONFIG_PATH := $(LIBDIR)/pkgconfig
export EMFOR_LIBRARY   := $(SHLIB)
RPATH         := -Wl,-rpath,$(LIBDIR)
PC            := pkg-config

# The conformance driver (tools/emfor-fixture): $(EMFOR_CONFORMANCE) [--cases F]
# -- <lookup program...>, with absolute paths: cargo runs from the repository root.
EMFOR_CONFORMANCE := cd $(EMFOR_ROOT) && cargo run -q -p emfor-fixture --bin emfor-conformance --
# Writes one case of a cases.json file to a store: <cases.json> <index> <out.mdb>.
EMFOR_FIXTURE     := cd $(EMFOR_ROOT) && cargo run -q -p emfor-fixture --bin emfor-fixture --

.PHONY: libemfor
# Always reinstall, so tests use the current Rust sources (cargo skips the
# build when nothing changed).
libemfor:
	cd $(EMFOR_ROOT)/clients/rust && cargo cinstall --release --features capi \
	    --prefix $(PREFIX) --libdir $(LIBDIR) --pkgconfigdir $(LIBDIR)/pkgconfig >/dev/null

$(SHLIB): libemfor
