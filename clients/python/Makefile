# Python wrapper tests: `make test` installs libemfor (clients/libemfor.mk,
# which also sets EMFOR_LIBRARY), runs the unit tests against a store written
# by emfor-fixture, and runs the conformance driver. Needs Python 3.9 or later.

include ../libemfor.mk

PYTHON ?= python3
BUILD  := $(CLIENTS_BUILD)/python
STORE  := $(BUILD)/state.mdb

.PHONY: test clean
test: libemfor
	mkdir -p $(BUILD)
	$(EMFOR_FIXTURE) $(EMFOR_ROOT)/clients/c/tests/fixture.json 0 $(STORE)
	EMFOR_STORE=$(STORE) EMFOR_MAX_STALENESS=60s PYTHONPATH=$(CURDIR) \
	    $(PYTHON) -m unittest discover -s tests -v
	$(EMFOR_CONFORMANCE) -- $(PYTHON) $(CURDIR)/tests/conformance_lookup.py
	@echo "PASS all Python tests"

clean:
	rm -rf $(BUILD)
