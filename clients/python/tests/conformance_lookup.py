"""Python lookup program for the shared conformance driver (tools/emfor-fixture,
emfor-conformance): <name> <affinity-key or ""> <count>, printing
"OK <address>" or "ERR <CODE> <message>" per lookup."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import emfor  # noqa: E402


def main():
    if len(sys.argv) != 4:
        sys.exit("usage: conformance_lookup.py <name> <affinity-key> <count>")
    name, key, count = sys.argv[1], sys.argv[2], int(sys.argv[3])
    for _ in range(count):
        try:
            print("OK " + emfor.lookup(name, key))
        except emfor.Error as e:
            print("ERR %s %s" % (e.code, e.message))


if __name__ == "__main__":
    main()
