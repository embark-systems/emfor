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
rate_limit = 100              # default; UDP responses a second per client network
```

An empty `[dns]` table uses the defaults. The server listens on UDP and
TCP. If an address cannot be bound, the daemon exits with status 1. At
most 128 TCP connections are served at once (more are closed right away),
and a TCP connection that sends no query for 5 seconds is closed. If a
listener stops while the daemon runs, the daemon exits with status 70 and
the service manager restarts it.

Only services named under the zone are answered. At start and on reload
the daemon logs a warning for each service outside the zone, and for each
service whose backends listen on different ports (A and AAAA answers carry
no port; use SRV or a client library for such services).

### Rate limit

An SRV answer can be many times the size of its query, so a DNS
server reachable from other hosts can be used to flood a third party
with forged-source UDP queries. The server therefore counts UDP
responses per client network (an IPv4 /24 or an IPv6 /56) and sends at
most `[dns] rate_limit` (100 by default) of them a second to each. Past
the limit, every other response is an empty one with the TC flag, which
is smaller than the query and makes a real client retry over TCP; the
rest are not sent. Clients on a loopback address and TCP are never
limited, so the default setup (`listen` on 127.0.0.1, queried by the
host's own resolver) is not affected. `emfor_dns_rate_limited_total`
counts the limited responses; if a resolver of your site that forwards
to emfor shows up there, raise `rate_limit`, or set `0` to turn the
limit off.

At start the daemon logs a warning for each `listen` address that is
not a loopback address: any host that can reach it can look up every
service. Limit who can reach that port with a firewall.

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
| A name outside the zone, or a class other than `IN` | `REFUSED`. |
| EDNS version above 0 | `BADVERS`. |

The TTL is the service's `dns_ttl`, or `[dns] ttl` (1 second by default).
The zone has a synthesized SOA whose negative-caching TTL is also
`[dns] ttl`.

## SRV records

SRV answers carry each backend's port and list every candidate, chosen
as for A and AAAA answers: the primaries whose last check passed, else the
[backups](05-backend-selection.md#backup-backends) whose last check
passed, else every healthy primary when all of them are failing (see [A
backend whose checks start
failing](05-backend-selection.md#a-backend-whose-checks-start-failing)),
else every healthy backup. An answer never lists primaries and backups
together while any backend is healthy. Or,
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
truncated (TC) and the resolver retries over TCP. A name close to the
255-byte limit can make even a single A record or the SOA too big for 512
bytes; that reply is truncated too.

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

## Using it with nginx

Open-source nginx has no active health checks (only `max_fails`), and its
upstream list is fixed until a reload. With nginx 1.27.3 or later it can
take the list from emfor's SRV answers instead (`resolve` and `service=` were
commercial-only before that release), so a backend that fails its checks, a
backend you drain and a weight you change all reach nginx within a few
seconds (emfor's own check interval first, then the one second of `valid`),
without a reload:

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;    # emfor's DNS server, asked every second

upstream api {
    zone api 64k;                              # required with `resolve`
    server api.svc.emfor service=http resolve; # asks _http._tcp.api.svc.emfor
}

server {
    listen 80;
    location / { proxy_pass http://api; }
}
```

The `resolver` points at emfor directly (`[dns] listen`, port 5300 by
default), not at the host resolver: nginx has its own resolver cache, and
`valid=1s` makes it ask again every second. `service=http` is only the name
prefix (`_http._tcp`); emfor answers `_<anything>._tcp.<service>`
([SRV records](#srv-records)), so the port comes from each record.

What nginx does with the answer, as checked with nginx 1.30.6 (stable) and
1.31.8 (mainline):

- **Health**: a backend whose checks fail is not in the answer, and nginx
  stops sending to it; when it recovers, it returns. The test waits up to
  10 seconds for each change; with a 200 ms check interval it took about 3.
- **`drain = true`** ([chapter 5](05-backend-selection.md#draining-a-backend)):
  edit emfor's config and reload emfor; nginx is not touched, and the
  backend stops getting new requests. (What happens to a connection nginx
  already holds open to it was not tried.)
- **Priorities**: nginx takes the records with the lowest priority number as
  its servers and the others as `backup`. With `balance = "failover"` the
  first healthy backend is the only server and the second its backup;
  `locality` and `fastest` use priority 0 and 1 the same way.
- **Weights** of `balance = "weighted"` are followed: with weights 3 and 1,
  60 of 80 requests went to the first backend. (nginx's documentation does
  not say it uses the SRV weight.)
- **When emfor cannot answer**: a service with no backend to offer
  (`on_all_down = "closed"` and everything down, or every backend drained)
  gets `SERVFAIL`, and so does a stopped emfor. nginx logs `could not be
  resolved` and **keeps the last list it had**, so it goes on sending
  to those backends. The lookup fails for nginx's clients only through
  nginx's own `proxy_next_upstream` and `max_fails`.
- **HTTPS upstreams**: a record's target is a name like
  `10-0-0-11.api.svc.emfor`. Set `proxy_ssl_name` (and
  `proxy_ssl_server_name on`) to the name the backend's certificate has. This
  was not tried.

**Older nginx.** Before 1.27.3 the `service=` parameter is rejected at
`nginx -t` (`invalid parameter "service=http"`, seen with 1.26.3). Without
it, use a variable in `proxy_pass`, which makes nginx resolve the name at run
time, and give every backend of the service the same port:

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;
server {
    listen 80;
    location / {
        set $backend api.svc.emfor;
        proxy_pass http://$backend:8080;
    }
}
```

An A answer holds one backend, chosen by `balance`, so nginx sends all
requests of a second to that one (it keeps the answer for `valid`); the
rotation is across seconds, not across requests. Both backends were reached
in the test; how evenly was not measured.

[tests/nginx/](../../tests/nginx/run.sh) runs this on a given nginx image
(`tests/nginx/run.sh nginx:stable`): the checks above, and the older nginx
case. It is part of the CI.

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
