# Go wrapper tests: `make test` installs libemfor (clients/libemfor.mk), runs
# go vet and go test against a store written by emfor-fixture, and runs the
# conformance driver against cmd/emfor-conformance-lookup. Needs Go with cgo.

include ../libemfor.mk

BUILD := $(CLIENTS_BUILD)/go
STORE := $(BUILD)/state.mdb
# Binaries find libemfor in the test prefix without LD_LIBRARY_PATH.
export CGO_LDFLAGS := $(RPATH)
export CGO_ENABLED := 1

.PHONY: test clean
test: libemfor
	mkdir -p $(BUILD)
	go vet ./...
	$(EMFOR_FIXTURE) $(EMFOR_ROOT)/clients/c/tests/fixture.json 0 $(STORE)
	EMFOR_STORE=$(STORE) EMFOR_MAX_STALENESS=60s go test -count=1 ./...
	go build -o $(BUILD)/emfor-conformance-lookup ./cmd/emfor-conformance-lookup
	$(EMFOR_CONFORMANCE) -- $(BUILD)/emfor-conformance-lookup
	@echo "PASS all Go tests"

clean:
	rm -rf $(BUILD)
