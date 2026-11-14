#!/bin/sh
# Usage: tests/check_header.sh <emfor.h>
# Fails when the header defines a macro whose name does not start with
# `EMFOR_`, such as an internal Rust constant that cbindgen picked up.
set -eu
header="$1"
if [ ! -f "$header" ]; then echo "FAIL no header $header"; exit 1; fi
macros=$(grep -E '^#define ' "$header" | awk '{print $2}')
echo "macros: $(echo $macros)"
bad=$(echo "$macros" | grep -v '^EMFOR_' | grep -v '^$' || true)
if [ -n "$bad" ]; then echo "FAIL unprefixed macros in $header: $(echo $bad)"; exit 1; fi
echo "PASS header"
