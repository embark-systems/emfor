# PHP extension tests: `make test` installs libemfor (clients/libemfor.mk),
# builds the extension with phpize in clients/build/php (phpize writes its own
# Makefile, so it stays out of this directory), runs tests/emfor_test.php
# against a store written by emfor-fixture, and runs the conformance driver.
# Needs PHP 8.1 or later with its development files (phpize, php-config).

include ../libemfor.mk

PHPIZE ?= phpize
PHP    ?= php
BUILD  := $(CLIENTS_BUILD)/php
STORE  := $(BUILD)/state.mdb
SRC    := config.m4 emfor.c emfor_arginfo.h
EXT    := $(BUILD)/src/modules/emfor.so
RUN    := $(PHP) -d extension=$(EXT)

.PHONY: test build clean
build: libemfor
	mkdir -p $(BUILD)/src
	cp $(SRC) $(BUILD)/src/
	cd $(BUILD)/src && $(PHPIZE) >/dev/null && ./configure --enable-emfor >/dev/null
	$(MAKE) -C $(BUILD)/src CFLAGS="-O2 -Wall -Wextra -Werror -Wno-unused-parameter"

test: build
	$(EMFOR_FIXTURE) $(EMFOR_ROOT)/clients/c/tests/fixture.json 0 $(STORE)
	EMFOR_STORE=$(STORE) EMFOR_MAX_STALENESS=60s $(RUN) tests/emfor_test.php
	$(EMFOR_CONFORMANCE) -- $(RUN) $(CURDIR)/tests/conformance_lookup.php
	@echo "PASS all PHP tests"

clean:
	rm -rf $(BUILD)
