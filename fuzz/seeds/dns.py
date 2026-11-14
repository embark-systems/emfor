#!/usr/bin/env python3
"""Write the seed inputs of the `dns` fuzz target to fuzz/seeds/dns/.

Each seed is a mode byte and a DNS request, the queries of the DNS server's
tests in crates/emfor-daemon/src/dns.rs. Mode bit 0 sends the request over
TCP (else UDP), bit 1 asks in the longest zone (else svc.emfor), bit 2 asks
before the first layout (see emfor_daemon::dns::fuzz_request, which also
lists the services).

Run from anywhere: python3 fuzz/seeds/dns.py
"""

import struct
from pathlib import Path

OUT = Path(__file__).resolve().parent / "dns"

UDP, TCP, LONG_ZONE, NOT_READY = 0, 1, 2, 4
A, SOA, TXT, AAAA, SRV = 1, 6, 16, 28, 33
IN, CH = 1, 3


def name(text):
    """Wire form of a name; `\\.` is a dot inside a label."""
    labels, label, i = [], b"", 0
    while i < len(text):
        c = text[i]
        if c == "\\" and i + 1 < len(text):
            label += text[i + 1].encode()
            i += 2
            continue
        if c == ".":
            labels.append(label)
            label = b""
        else:
            label += c.encode()
        i += 1
    if label:
        labels.append(label)
    return b"".join(bytes([len(l)]) + l for l in labels) + b"\x00"


def query(qname, qtype, qclass=IN, edns=None, ident=9, questions=None):
    """A query with recursion desired; `edns` is (payload size, version)."""
    qs = questions if questions is not None else [(qname, qtype, qclass)]
    body = b"".join(name(n) + struct.pack(">HH", t, c) for n, t, c in qs)
    additional = 0
    if edns is not None:
        size, version = edns
        body += b"\x00" + struct.pack(">HHBBHH", 41, size, 0, version, 0, 0)
        additional = 1
    header = struct.pack(">HHHHHH", ident, 0x0100, len(qs), 0, 0, additional)
    return header + body


LONG = "a" * 63 + "." + "b" * 63 + "." + "c" * 63 + "." + "d" * 50 + ".svc.emfor"
ZONE_LEN = 253 - len("hostmaster.")


def long_zone():
    labels, left = [], ZONE_LEN
    while left > 0:
        n = min(60, left)
        labels.append("a" * n)
        left -= n + 1
    return ".".join(labels)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    seeds = {
        "a": (UDP, query("api.svc.emfor.", A)),
        "aaaa": (UDP, query("api.svc.emfor.", AAAA)),
        "a-edns": (UDP, query("api.svc.emfor.", A, edns=(1232, 0))),
        "a-tcp": (TCP, query("api.svc.emfor.", A)),
        "a-closed-all-down": (UDP, query("db.svc.emfor.", A)),
        "a-locality": (UDP, query("web.svc.emfor.", A)),
        "txt-nodata": (UDP, query("api.svc.emfor.", TXT)),
        "a-nxdomain": (UDP, query("nope.svc.emfor.", A)),
        "soa": (UDP, query("svc.emfor.", SOA)),
        "a-outside-zone": (UDP, query("example.org.", A)),
        "srv": (UDP, query("api.svc.emfor.", SRV)),
        "srv-owner": (UDP, query("_http._tcp.api.svc.emfor.", SRV)),
        "srv-tcp-node": (UDP, query("_tcp.api.svc.emfor.", A)),
        "srv-target-a": (UDP, query("10-0-0-11.api.svc.emfor.", A)),
        "srv-target-aaaa": (
            UDP,
            query("2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor.", AAAA),
        ),
        "srv-many-512": (UDP, query("many.svc.emfor.", SRV)),
        "srv-many-edns": (UDP, query("many.svc.emfor.", SRV, edns=(1232, 0))),
        "srv-many-tcp": (TCP, query("many.svc.emfor.", SRV)),
        "class-ch": (UDP, query("api.svc.emfor.", A, qclass=CH)),
        "edns-version-1": (UDP, query("api.svc.emfor.", A, edns=(1232, 1))),
        "label-with-dot": (UDP, query("10-0-0-11\\.api.svc.emfor.", A)),
        "long-name-a": (UDP, query(LONG + ".", A, edns=(512, 0))),
        "long-name-srv": (UDP, query(LONG + ".", SRV)),
        "two-questions": (
            UDP,
            query(None, None, questions=[("api.svc.emfor.", A, IN), ("db.svc.emfor.", A, IN)]),
        ),
        "long-zone-txt": (UDP | LONG_ZONE, query(long_zone() + ".", TXT)),
        "long-zone-srv": (UDP | LONG_ZONE, query("api." + long_zone() + ".", SRV)),
        "not-ready": (UDP | NOT_READY, query("api.svc.emfor.", A)),
    }
    for name_, (mode, data) in seeds.items():
        (OUT / name_).write_bytes(bytes([mode]) + data)
    print(f"wrote {len(seeds)} seeds to {OUT}")


if __name__ == "__main__":
    main()
