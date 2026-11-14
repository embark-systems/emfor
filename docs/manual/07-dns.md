# 7. DNS

[Manual](README.md) · Previous: [6. Client libraries](06-clients.md) · Next: [8. Operations](08-operations.md)

Programs that cannot use a client library (a third-party binary, a
language without a client, a `CGO_ENABLED=0` Go build) can resolve service
names over DNS instead. emfor's DNS server answers only its own zone, on a
loopback port; the host's normal resolver forwards that zone to it.

```
program --> host resolver (dnsmasq, unbound, systemd-resolved) --> other names: upstream DNS
                     |
                     +-- *.svc.emfor --> emfor on 127.0.0.1:5300
```

emfor is never the host's general DNS server, so if the daemon stops, only
names in its zone stop resolving.

## Turning it on

```toml
[dns]
listen = ["127.0.0.1:5300"]   # default; not 5353, which is mDNS
zone = "svc.emfor"            # default
ttl = "1s"                    # default
```

An empty `[dns]` table uses the defaults. The server listens on UDP and
TCP. If an address cannot be bound, the daemon exits with status 1.

Only services named under the zone are answered. At start and on reload
the daemon logs a warning for each service outside the zone, and for each
service whose backends listen on different ports (A and AAAA answers carry
no port; use SRV or a client library for such services).

Check it directly before you configure a resolver:

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A          # Linux, macOS
drill -p 5300 api.svc.emfor @127.0.0.1           # FreeBSD
```

## Answers

| Query | Answer |
| --- | --- |
| A / AAAA for a service | One record: a backend of that family chosen by `balance` ([chapter 5](05-backend-selection.md#dns-differences)), with the `aa` flag. |
| A / AAAA, no backend of that family | `NOERROR` with no records (NODATA). |
| A / AAAA / SRV, no healthy backend and `on_all_down = "closed"` | `SERVFAIL`. |
| SRV for a service, or `_<anything>._tcp.<service>` | One SRV record per candidate, addresses in the additional section. |
| Other types for a service | `NOERROR` with no records. |
| A name under the zone that is no service | `NXDOMAIN`. |
| A name outside the zone | `REFUSED`. |

The TTL is the service's `dns_ttl`, or `[dns] ttl` (1 second by default).
The zone has a synthesized SOA whose negative-caching TTL is also
`[dns] ttl`.

## SRV records

SRV answers carry each backend's port and list every candidate: the
healthy backends whose last check passed, or every healthy backend when
all of them are failing (see [A backend whose checks start
failing](05-backend-selection.md#a-backend-whose-checks-start-failing)), or,
when none is healthy and `on_all_down` is `open`, the `unknown` backends
(those whose last check passed first) before all of them (see [When no
backend is healthy](05-backend-selection.md#when-no-backend-is-healthy)).
IPv4 and IPv6 backends are listed together. Go's
`net.LookupSRV("http", "tcp", "api.svc.emfor")`, gRPC and similar clients
ask for `_http._tcp.api.svc.emfor`, which emfor answers.

| `balance` | Priority | Weight | Order |
| --- | --- | --- | --- |
| `failover` | 0, 1, 2 … in config order | 1 | config order |
| `weighted` | 0 | the backend's `weight` | config order |
| `locality` | 0 in the host's `site`, 1 elsewhere | 1 | rotates per query |
| `fastest` | 0 in the pool, 1 outside it | 1 | rotates per query |
| `round-robin`, `sticky` | 0 | 1 | rotates per query |

While a candidate is in [slow
start](05-backend-selection.md#slow-start), every method but `failover`
sets each weight to the backend's reduced weight: its `weight` (for
`weighted`) or 1, times its step of 1 to 10.

Each record's target is a name derived from the backend's address under
the service name, which emfor resolves whatever the backend's health:

- `10.0.0.11` → `10-0-0-11.api.svc.emfor`
- `2001:db8::11` → `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor`

Over UDP a reply stays within the size the client advertises with EDNS (at
most 1232 bytes), or 512 bytes without EDNS. When the addresses do not fit,
they are left out; when even the SRV records do not fit, the reply is
truncated (TC) and the resolver retries over TCP.

## Forwarding from the host resolver

Pick the resolver your hosts use. These snippets use the defaults; adjust
the zone and port if you changed them.

**dnsmasq** (`/etc/dnsmasq.d/emfor.conf`, then `systemctl restart dnsmasq`):

```
server=/svc.emfor/127.0.0.1#5300
```

**unbound**, also FreeBSD's `local_unbound` (`/etc/unbound/unbound.conf.d/emfor.conf`
or `/var/unbound/conf.d/emfor.conf`):

```
server:
    do-not-query-localhost: no
    domain-insecure: "svc.emfor"

forward-zone:
    name: "svc.emfor"
    forward-addr: 127.0.0.1@5300
```

Both `server:` lines are required: unbound does not query localhost by
default, and a validating unbound would reject the unsigned zone.

**systemd-resolved** (`/etc/systemd/resolved.conf.d/emfor.conf`, systemd 246
or later, then `systemctl restart systemd-resolved`):

```
[Resolve]
DNS=127.0.0.1:5300
Domains=~svc.emfor
```

**macOS** (development only; file `/etc/resolver/svc.emfor`):

```
nameserver 127.0.0.1
port 5300
```

Then test through the system resolver: `getent hosts api.svc.emfor` on
Linux and FreeBSD, `dscacheutil -q host -a name api.svc.emfor` on macOS.
[docs/deploy/dns-resolver.md](../deploy/dns-resolver.md) has the verified
recipes and more checks.

## Caching

emfor's short TTL lets a failed backend drop out of answers within about a
second. Caches can stretch that:

- Leave the resolver's minimum cache TTL at 0 (dnsmasq `min-cache-ttl`,
  unbound `cache-min-ttl`).
- A resolver serves repeated queries from its cache for the TTL, so all
  programs on the host get the same round-robin answer for that long. Lower
  `dns_ttl`, down to `0s`, to rotate more often.
- Negative answers (a name not yet configured) are cached for `[dns] ttl`.
- Application runtimes may cache on their own (the JVM's
  `networkaddress.cache.ttl`); emfor cannot shorten those.
