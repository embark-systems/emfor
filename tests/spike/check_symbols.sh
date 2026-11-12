#!/bin/sh
# Usage: tests/spike/check_symbols.sh <shared library> <allowed prefix>
# Fails when the library exports a defined symbol not starting with the
# prefix (macOS adds a leading underscore, which is stripped first).
set -eu
lib="$1"; prefix="$2"
if [ "$(uname)" = Darwin ]; then
    syms=$(nm -gU "$lib" | awk '{print $3}' | sed 's/^_//')
else
    syms=$(nm -D --defined-only "$lib" | awk '{print $3}')
fi
echo "exported: $(echo $syms)"
bad=$(echo "$syms" | grep -v "^$prefix" | grep -v '^$' || true)
if [ -n "$bad" ]; then echo "FAIL unexpected exports: $(echo $bad)"; exit 1; fi
echo "PASS symbols"
