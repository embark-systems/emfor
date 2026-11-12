#!/bin/sh
# Usage: tests/check_symbols.sh <shared library>
# Fails when the library exports a defined symbol not starting with
# `emfor_`, such as the bundled LMDB's `mdb_*` (macOS adds a leading
# underscore, which is stripped first).
set -eu
lib="$1"
if [ "$(uname)" = Darwin ]; then
    syms=$(nm -gU "$lib" | awk '{print $3}' | sed 's/^_//')
else
    syms=$(nm -D --defined-only "$lib" | awk '{print $3}')
fi
echo "exported: $(echo $syms)"
bad=$(echo "$syms" | grep -v '^emfor_' | grep -v '^$' || true)
if [ -n "$bad" ]; then echo "FAIL unexpected exports: $(echo $bad)"; exit 1; fi
echo "PASS symbols"
