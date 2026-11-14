#!/usr/bin/env python3
"""Write the seed inputs of the `plugins` fuzz target to fuzz/seeds/plugins/.

One seed or more per plugin. Each seed is a byte picking the plugin (its index in KINDS in
fuzz_targets/plugins.rs), a byte picking its options, and a well-formed
answer of the backend. In fuzzing builds request identifiers and nonces are
zero, so these answers match the requests. UDP answers are datagrams, each
prefixed with its length as two bytes, big-endian.

Run from anywhere: python3 fuzz/seeds/plugins.py
"""

import hashlib
import hmac
import struct
from pathlib import Path

OUT = Path(__file__).resolve().parent / "plugins"

REDIS, LDAP, DNS, RADIUS, SIP, DIAMETER, SMTP, AMQP, MONGODB, GRPC = range(10)


def datagrams(*ds):
    return b"".join(struct.pack(">H", len(d)) + d for d in ds)


# redis


def bulk(s):
    return b"$%d\r\n%s\r\n" % (len(s), s)


def redis():
    info = bulk(b"# Replication\r\nrole:slave\r\nmaster_last_io_seconds_ago:1\r\n")
    replica_role = (
        b"*5\r\n" + bulk(b"slave") + bulk(b"10.0.0.1") + b":6379\r\n" + bulk(b"connected") + b":100\r\n"
    )
    return {
        "redis-pong": (0, b"+PONG\r\n"),
        "redis-master": (1, b"+PONG\r\n*3\r\n" + bulk(b"master") + b":100\r\n*0\r\n"),
        "redis-replica": (2, b"+PONG\r\n" + replica_role + info),
        "redis-auth": (3, b"+OK\r\n+PONG\r\n"),
        "redis-error": (0, b"-NOAUTH Authentication required.\r\n"),
    }


# ldap: BindResponse, messageID 1, resultCode success


def ldap():
    ok = bytes.fromhex("300c02010161070a010004000400")
    bad = bytes.fromhex("300c02010161070a013104000400")
    return {"ldap-success": (0, ok), "ldap-invalid-credentials": (1, bad)}


# dns


def name(labels):
    return b"".join(bytes([len(l)]) + l for l in labels) + b"\x00"


def dns_response(qname, qtype, answers, rcode=0, tcp=False):
    header = struct.pack(">HHHHHH", 0, 0x8180 | rcode, 1, len(answers), 0, 0)
    question = name(qname) + struct.pack(">HH", qtype, 1)
    records = b"".join(
        b"\xc0\x0c" + struct.pack(">HHIH", t, 1, 60, len(rdata)) + rdata for t, rdata in answers
    )
    msg = header + question + records
    return datagrams(msg) if not tcp else struct.pack(">H", len(msg)) + msg


def dns():
    example = [b"example", b"org"]
    mx = struct.pack(">H", 10) + name([b"mx", b"example", b"org"])
    txt = b"\x05hello"
    soa = name([b"a", b"root-servers", b"net"]) + name([b"nstld"]) + struct.pack(">IIIII", 1, 2, 3, 4, 5)
    srv = struct.pack(">HHH", 10, 5, 5060) + name([b"sip", b"example", b"org"])
    return {
        "dns-a": (0, dns_response(example, 1, [(1, bytes([127, 0, 0, 1]))])),
        "dns-mx": (1, dns_response(example, 15, [(15, mx)])),
        "dns-txt-tcp": (2, dns_response(example, 16, [(16, txt)], tcp=True)),
        "dns-soa-nxdomain": (3, dns_response([], 6, [(6, soa)], rcode=3)),
        "dns-srv": (4, dns_response([b"_sip", b"_udp", b"example", b"org"], 33, [(33, srv)])),
    }


# radius: the request authenticator is zero, the secret "testing123"

SECRET = b"testing123"


def radius_packet(code, attrs=b"", message_authenticator=False):
    if message_authenticator:
        attrs = attrs + bytes([80, 18]) + bytes(16)
    length = 20 + len(attrs)
    if message_authenticator:
        unsigned = bytes([code, 0]) + struct.pack(">H", length) + bytes(16) + attrs
        mac = hmac.new(SECRET, unsigned, hashlib.md5).digest()
        attrs = attrs[:-16] + mac
    head = bytes([code, 0]) + struct.pack(">H", length)
    auth = hashlib.md5(head + bytes(16) + attrs + SECRET).digest()
    return head + auth + attrs


def radius():
    reply = bytes([18, 7]) + b"hello"
    return {
        "radius-accept": (0, datagrams(radius_packet(2))),
        "radius-accept-message-authenticator": (1, datagrams(radius_packet(2, message_authenticator=True))),
        "radius-reject": (2, datagrams(radius_packet(3, reply, message_authenticator=True))),
    }


# sip: branch, tags and Call-ID are all zeros


def sip_response(via, status=b"200 OK"):
    return (
        b"SIP/2.0 " + status + b"\r\n"
        b"Via: SIP/2.0/" + via + b" 127.0.0.1:40000;branch=z9hG4bK0000000000000000;rport=40000\r\n"
        b"From: <sip:emfor@emfor.invalid>;tag=0000000000\r\n"
        b"To: <sip:127.0.0.1:4242>;tag=abc\r\n"
        b"Call-ID: 00000000000000000000@emfor\r\n"
        b"CSeq: 1 OPTIONS\r\n"
        b"Content-Length: 0\r\n\r\n"
    )


def sip():
    return {
        "sip-udp-200": (0, datagrams(sip_response(b"UDP"))),
        "sip-tcp-200": (1, sip_response(b"TCP")),
        "sip-tcp-503": (2, sip_response(b"TCP", b"503 Service Unavailable")),
        # A provisional response first: its Content-Length is added to
        # the head before the final one is read.
        "sip-tcp-100-200": (1, sip_response(b"TCP", b"100 Trying") + sip_response(b"TCP")),
    }


# diameter: the identifiers start at zero and grow by one per request


def avp(code, data, vendor=None):
    flags = 0x40 | (0x80 if vendor is not None else 0)
    head_len = 12 if vendor is not None else 8
    head = struct.pack(">I", code) + bytes([flags]) + (head_len + len(data)).to_bytes(3, "big")
    if vendor is not None:
        head += struct.pack(">I", vendor)
    return head + data + bytes(-len(data) % 4)


def diameter_message(command, avps, n=0, flags=0):
    """An answer to the `n`th request: its identifiers are both `n`."""
    body = b"".join(avps)
    length = 20 + len(body)
    return (
        bytes([1])
        + length.to_bytes(3, "big")
        + bytes([flags])
        + command.to_bytes(3, "big")
        + struct.pack(">III", 0, n, n)
        + body
    )


def diameter():
    common = [
        avp(268, struct.pack(">I", 2001)),
        avp(264, b"hss.example.org"),
        avp(296, b"example.org"),
    ]
    cea = diameter_message(257, common + [avp(258, struct.pack(">I", 16777251))])
    cea_vendor = diameter_message(
        257,
        common
        + [avp(260, avp(266, struct.pack(">I", 10415)) + avp(258, struct.pack(">I", 16777238)))],
    )
    dwa = diameter_message(280, common, n=1)
    dpa = diameter_message(282, common, n=2)
    refused = diameter_message(257, [avp(268, struct.pack(">I", 3010))] + common[1:])
    return {
        "diameter-ok": (0, cea + dwa + dpa),
        "diameter-vendor": (1, cea_vendor + dwa + dpa),
        "diameter-unknown-peer": (0, refused),
    }


# smtp


def smtp():
    return {
        "smtp-ok": (0, b"220 mx.example.org ESMTP\r\n250-mx.example.org\r\n250 SIZE 10240000\r\n221 bye\r\n"),
        "smtp-busy": (1, b"421 too busy\r\n"),
    }


# amqp


def short(s):
    return bytes([len(s)]) + s


def long_bytes(s):
    return struct.pack(">I", len(s)) + s


def method(class_id, method_id, args):
    payload = struct.pack(">HH", class_id, method_id) + args
    return bytes([1]) + struct.pack(">HI", 0, len(payload)) + payload + b"\xce"


def amqp():
    props = short(b"product") + b"S" + long_bytes(b"RabbitMQ")
    start = method(10, 10, bytes([0, 9]) + long_bytes(props) + long_bytes(b"PLAIN AMQPLAIN") + long_bytes(b"en_US"))
    tune = method(10, 30, struct.pack(">HIH", 2047, 131072, 60))
    open_ok = method(10, 41, short(b""))
    close_ok = method(10, 51, b"")
    refused = method(10, 50, struct.pack(">H", 530) + short(b"NOT_ALLOWED - vhost not found") + struct.pack(">HH", 10, 40))
    return {
        "amqp-ok": (0, start + tune + open_ok + close_ok),
        "amqp-vhost-refused": (1, start + tune + refused),
    }


# mongodb: the hello is request 1


def bson(fields):
    body = b""
    for key, value in fields:
        k = key.encode() + b"\x00"
        if isinstance(value, bool):
            body += b"\x08" + k + bytes([value])
        elif isinstance(value, float):
            body += b"\x01" + k + struct.pack("<d", value)
        elif isinstance(value, int):
            body += b"\x10" + k + struct.pack("<i", value)
        elif isinstance(value, str):
            v = value.encode() + b"\x00"
            body += b"\x02" + k + struct.pack("<i", len(v)) + v
        elif isinstance(value, list):
            body += b"\x04" + k + bson([(str(i), x) for i, x in enumerate(value)])
    return struct.pack("<i", len(body) + 5) + body + b"\x00"


def op_msg(response_to, doc):
    body = struct.pack("<I", 0) + b"\x00" + doc
    return struct.pack("<iiii", 16 + len(body), 7, response_to, 2013) + body


def mongodb():
    primary = bson([("isWritablePrimary", True), ("setName", "rs0"), ("hosts", ["a:27017", "b:27017"]), ("ok", 1.0)])
    secondary = bson([("isWritablePrimary", False), ("secondary", True), ("setName", "rs0"), ("ok", 1.0)])
    error = bson([("ok", 0.0), ("errmsg", "not authorized"), ("code", 13), ("codeName", "Unauthorized")])
    return {
        "mongodb-primary": (1, op_msg(1, primary)),
        "mongodb-secondary": (2, op_msg(1, secondary)),
        "mongodb-standalone": (0, op_msg(1, bson([("isWritablePrimary", True), ("ok", 1.0)]))),
        "mongodb-auth-error": (3, op_msg(1, primary) + op_msg(2, error)),
    }


# grpc: HTTP/2 frames on stream 1, the client's first request


def frame(kind, flags, stream, payload):
    return len(payload).to_bytes(3, "big") + bytes([kind, flags]) + struct.pack(">I", stream) + payload


def hpack_literal(name_, value):
    """A literal header field without indexing, with a new name (RFC 7541 6.2.2)."""
    return b"\x00" + bytes([len(name_)]) + name_ + bytes([len(value)]) + value


def grpc_response(status=None, grpc_status=b"0", grpc_message=None, http_status=b"\x88"):
    """`http_status` is the HPACK of :status; 0x88 is the static entry 200."""
    # The server's SETTINGS and its ACK of the client's, as a real server
    # (h2) writes them.
    settings = frame(4, 0, 0, b"") + frame(4, 1, 0, b"")
    headers = http_status + b"\x5f\x10application/grpc"
    out = settings + frame(1, 0x4, 1, headers)
    if status is not None:
        message = b"\x08" + bytes([status])
        out += frame(0, 0, 1, b"\x00" + struct.pack(">I", len(message)) + message)
    trailers = hpack_literal(b"grpc-status", grpc_status)
    if grpc_message is not None:
        trailers += hpack_literal(b"grpc-message", grpc_message)
    return out + frame(1, 0x5, 1, trailers)


def grpc():
    return {
        "grpc-serving": (0, grpc_response(status=1)),
        "grpc-not-serving": (1, grpc_response(status=2)),
        "grpc-unimplemented": (0, grpc_response(grpc_status=b"12", grpc_message=b"unknown%20service")),
        "grpc-http-503": (0, grpc_response(http_status=b"\x48\x03503")),
    }


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    seeds = {}
    for kind, make in [
        (REDIS, redis),
        (LDAP, ldap),
        (DNS, dns),
        (RADIUS, radius),
        (SIP, sip),
        (DIAMETER, diameter),
        (SMTP, smtp),
        (AMQP, amqp),
        (MONGODB, mongodb),
        (GRPC, grpc),
    ]:
        for name_, (preset, reply) in make().items():
            seeds[name_] = bytes([kind, preset]) + reply
    for name_, data in seeds.items():
        (OUT / name_).write_bytes(data)
    print(f"wrote {len(seeds)} seeds to {OUT}")


if __name__ == "__main__":
    main()
