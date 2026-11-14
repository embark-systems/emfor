#!/usr/bin/env python3
"""Writes the seeds of the peers fuzz target to seeds/peers/.

Each seed is one byte that picks the input's form (see
emfor_daemon::peers::fuzz_decode), then the input: 0, an EMP1 datagram;
1, an EMP2 datagram; 2, a payload that the target encrypts itself. All are
signed or encrypted with the target's secret, 32 bytes of 0x5a. Run from
fuzz/: python3 seeds/peers.py
"""
import hashlib
import hmac
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "crates", "emfor-daemon", "tests"))
from emp2_kat import hkdf, seal  # noqa: E402  (plain-Python RFC 5869 / RFC 8439)

SECRET = bytes([0x5A] * 32)
T = 1763000000000
PAYLOADS = {
    "heartbeat": '{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":1,"ts":%d,"kind":"heartbeat","off":["a513a047","fa6f63b4"]}' % T,
    "down": '{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":2,"ts":%d,"kind":"down","service":"api.svc.emfor","digest":"0123456789abcdef0123456789abcdef","backend":"10.0.0.11:8080","reason":"connect: refused"}' % T,
    "result": '{"from":"10.0.0.6:5302","site":"dc1","boot":"0000000000000001","seq":3,"ts":%d,"kind":"result","service":"api.svc.emfor","digest":"0123456789abcdef0123456789abcdef","backend":"10.0.0.11:8080","outcome":"down","reason":"timeout","interval_ms":2000}' % T,
}

out = os.path.join(os.path.dirname(__file__), "peers")
os.makedirs(out, exist_ok=True)
key = hkdf(b"emfor peers", SECRET, b"EMP2 chacha20-poly1305", 32)
for name, text in PAYLOADS.items():
    p = text.encode()
    emp1 = b"EMP1" + hmac.new(SECRET, b"EMP1" + p, hashlib.sha256).digest() + p
    nonce = bytes(range(12))
    emp2 = b"EMP2" + nonce + seal(key, nonce, p, b"EMP2")
    for prefix, body, suffix in [(0, emp1, "emp1"), (1, emp2, "emp2"), (2, p, "payload")]:
        with open(os.path.join(out, f"{name}-{suffix}"), "wb") as f:
            f.write(bytes([prefix]) + body)
