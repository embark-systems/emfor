# `make -C clients test` runs every client's tests: the C library and the Go,
# Python, Perl and PHP wrappers. Each also runs alone: make -C clients/<lang> test.

CLIENTS := c go python perl php

.PHONY: test $(CLIENTS)
test: $(CLIENTS)

# One after another: they share clients/build/prefix.
go: c
python: go
perl: python
php: perl

$(CLIENTS):
	$(MAKE) -C $@ test
