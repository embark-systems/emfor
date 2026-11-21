# 10. DNS

[Contents](README.md) · Previous: [9. Client libraries](09-clients.md) · Next: [11. Peer groups](11-peer-groups.md)

The daemon can answer DNS queries for its services, so that programs without
a client library (a proxy, a shell script, a program you cannot change) get a
healthy backend too. It answers only its own zone and never forwards other
names: it sits behind the host's resolver, not in place of it.

```text
program --> host resolver (dnsmasq, unbound, systemd-resolved) --> other names: upstream DNS
                     |
                     +-- *.svc.emfor --> emfor on 127.0.0.1:5300
```

If the daemon stops, only names in its zone stop resolving.

## Turning it on

```toml
[dns]
listen = ["127.0.0.1:5300"]
zone = "svc.emfor"
ttl = "1s"
rate_limit = 100

[services."api.svc.emfor"]
check = "http"
backends = ["10.0.0.11:8080", "10.0.0.12:8080"]
```

Without a `[dns]` table no DNS socket is opened. With an empty one, every
key has the default shown.

| Key | Default | Rule |
| --- | --- | --- |
| `listen` | `["127.0.0.1:5300"]` | UDP and TCP on each address. Changing it needs a restart. |
| `zone` | `svc.emfor` | At most 242 characters. Services outside it are not answered; the daemon warns about each at start and reload. |
| `ttl` | `1s` | Whole seconds, 0–3600. Also the SOA minimum, so also how long a negative answer is cached. |
| `rate_limit` | `100` | UDP answers per second per client network (an IPv4 /24 or IPv6 /56), 0–1,000,000; `0` is no limit. Loopback clients are never limited. |

A service can set its own TTL with `dns_ttl = "0s"` (whole seconds, 0–3600;
not allowed in `[defaults]`).

At start the daemon warns when DNS listens on an address that is not
loopback (`DNS listens on X, which is not a loopback address: ...`), and for
each service whose backends use more than one port:
`service S has backends on different ports (8081, 8082); DNS A/AAAA answers carry no port, so use SRV records or an emfor client to get the port`.

To listen on port 53 under systemd, give the unit `CAP_NET_BIND_SERVICE`
([chapter 4](04-installing.md#the-systemd-unit)).

## The answers

Answers come from the daemon's memory, with the authoritative-answer (`aa`)
flag.

| Query | Answer |
| --- | --- |
| A on a service | One IPv4 backend, chosen by `balance` ([chapter 8](08-choosing-a-backend.md#dns-and-the-client-libraries-compared)). |
| AAAA on a service | The same for IPv6 backends. |
| A or AAAA, no backend of that family | NOERROR, no records, the SOA. |
| SRV on a service, or on `_<anything>._tcp.<service>` | One SRV record per candidate, both families, with the port; the addresses in the additional section. |
| Any other type on a service | NOERROR, no records, the SOA. |
| A service that is all down, `on_all_down = "closed"` | SERVFAIL. |
| A, AAAA or SRV on a service whose backends (of that family, for A and AAAA) are all drained, `open` included | SERVFAIL ([chapter 8](08-choosing-a-backend.md#drain)). |
| `<ip-label>.<service>` (an SRV target) | The backend's address, whatever its health. |
| SOA on the zone | `svc.emfor. 1 IN SOA ns.svc.emfor. hostmaster.svc.emfor. <serial> 3600 600 86400 1` (the serial is the daemon's start time). |
| A parent of a service name, or `_tcp.<service>` | NOERROR, no records, the SOA. |
| Another name in the zone | NXDOMAIN with the SOA. |
| A name outside the zone, or a class other than IN | REFUSED. |
| Before the daemon's first results | SERVFAIL (not cached like NXDOMAIN would be). |
| More than one question | FORMERR. An opcode other than QUERY gives NOTIMP; EDNS version above 0 gives BADVERS. |

Names are matched without regard to case and a trailing dot.

### SRV records

| `balance` | Priority | Weight | Order |
| --- | --- | --- | --- |
| `failover` | 0, 1, 2 … in config order | 1 | config order |
| `weighted` | 0 | the backend's `weight` | config order |
| `locality` | 0 at the host's `site`, 1 elsewhere (all 0 when none matches) | 1 | rotates |
| `fastest` | 0 in the fast pool, 1 outside (all 0 when the pool is empty) | 1 | rotates |
| `round-robin`, `sticky` | 0 | 1 | rotates |

While a candidate is in [slow start](08-choosing-a-backend.md#slow-start),
every method but `failover` gives each record its effective weight instead.

Target names are the backend's IP under the service name:

- `10.0.0.11` under `api.svc.emfor` is `10-0-0-11.api.svc.emfor`;
- `2001:db8::11` is `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor`
  (all eight groups, four digits each).

Over UDP an answer fits in the size the client offers with EDNS (at most
1232 bytes) or in 512 bytes without EDNS. When the addresses do not fit,
they are left out of the additional section; when the SRV records do not
fit, the TC flag tells the resolver to ask again over TCP. TCP takes at most
128 connections at once and closes one that sends no query for 5 seconds.

### Checking it

A query to the daemon directly, with `dig`:

```sh
$ dig @127.0.0.1 -p 5300 +short api.svc.emfor A
$ dig @127.0.0.1 -p 5300 api.svc.emfor SRV
$ dig @127.0.0.1 -p 5300 10-0-0-11.api.svc.emfor A
```

For the quick start of [chapter 2](02-quick-start.md), with one backend up and
one down, the daemon answered (checked with a small Python DNS client, as
`dig` was not installed in the test container):

| Query | Answer |
| --- | --- |
| `web.svc.emfor A` | `127.0.0.1`, TTL 1, `aa` set |
| `web.svc.emfor SRV` | `0 1 8080 127-0-0-1.web.svc.emfor.`, address in the additional section |
| `_http._tcp.web.svc.emfor SRV` | the same record |
| `127-0-0-2.web.svc.emfor A` (the backend that is down) | `127.0.0.2` |
| `svc.emfor SOA` | the SOA, TTL 1 |
| `nope.svc.emfor A` | NXDOMAIN with the SOA |
| `example.com A` | REFUSED, without `aa` |

### Rate limit

Over UDP, when a client network sends more than `rate_limit` queries in one
second, the answers over the limit are, in turn, an empty answer with the TC
flag set (a real resolver then asks over TCP, which is not limited) and no
answer at all. `emfor_dns_rate_limited_total{action="truncated"}` and
`{action="dropped"}` count them. The limit protects against address spoofing
when DNS listens on a non-loopback address; on `127.0.0.1` it never applies.

## Forwarding from the host resolver

Forward the zone, and only the zone, from the resolver your programs already
use. The snippets are those in [docs/deploy/resolver/](../deploy/resolver/).

**dnsmasq**, `/etc/dnsmasq.d/emfor.conf`, then restart dnsmasq:

```text
server=/svc.emfor/127.0.0.1#5300
```

**unbound** (Linux), or FreeBSD's `local_unbound` in
`/var/unbound/conf.d/emfor.conf`:

```text
server:
    do-not-query-localhost: no
    domain-insecure: "svc.emfor"

forward-zone:
    name: "svc.emfor"
    forward-addr: 127.0.0.1@5300
```

Both `server:` lines are needed: unbound does not query localhost by default,
and a validating unbound rejects the unsigned zone (SERVFAIL).

**systemd-resolved** (systemd 246 or later),
`/etc/systemd/resolved.conf.d/emfor.conf`, then
`systemctl restart systemd-resolved`:

```ini
[Resolve]
DNS=127.0.0.1:5300
Domains=~svc.emfor
```

**macOS** (development only), the file `/etc/resolver/svc.emfor`:

```text
nameserver 127.0.0.1
port 5300
```

Then test through the system resolver: `getent hosts api.svc.emfor` on Linux
and FreeBSD, `dscacheutil -q host -a name api.svc.emfor` on macOS. On macOS,
`dig` bypasses `/etc/resolver`.

According to [docs/deploy/dns-resolver.md](../deploy/dns-resolver.md), the
dnsmasq and unbound snippets are tested in Debian containers
(`tests/resolver/run.sh`), `local_unbound` on FreeBSD 15.1, systemd-resolved
on an Ubuntu VM with systemd 259, and macOS by hand. None of them was rerun
for this manual.

## Caching

emfor's short TTL lets a failed backend drop out of DNS answers within about
a second. Caches stretch that:

- Leave the resolver's minimum cache TTL at 0 (dnsmasq `min-cache-ttl`,
  unbound `cache-min-ttl`).
- A caching resolver answers repeated queries from its cache for the TTL, so
  every program on the host gets the same backend for that long. Lower
  `dns_ttl`, down to `0s`, to rotate more often.
- NXDOMAIN and empty answers are cached for `[dns] ttl` too: a name you have
  just added may stay negative for that long.
- SERVFAIL is cached briefly or not at all, so a `closed` service resolves
  again soon after a backend recovers.
- Some runtimes cache by themselves (the JVM's `networkaddress.cache.ttl`).
  emfor cannot shorten that.

## DNS or a client library?

Prefer a client library where you can: it needs no resolver setup, returns
the port, supports `sticky`, and keeps working from the store's last state
(with `on_all_down`) while the daemon restarts, whereas DNS gives no answer
at all while the daemon is down. DNS is the right choice for programs that
you cannot change, and for proxies that re-resolve names
([chapter 14](14-integrations.md)).
