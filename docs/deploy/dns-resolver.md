# Forwarding the emfor zone from the host resolver

emfor's DNS server answers only its own zone (`[dns] zone`, default `svc.emfor`) on a loopback port (`[dns] listen`, default `127.0.0.1:5300`). Programs don't query it directly. They use the host's normal resolver, and that resolver forwards just this zone to emfor. emfor is never the host's general DNS server, so if the daemon stops, only names in its zone stop resolving.

This guide uses the defaults: zone `svc.emfor`, `127.0.0.1:5300`, and a service named `api.svc.emfor`. If you change `zone` or `listen`, change the snippets to match.

Each snippet below is copied verbatim from a file in [resolver/](resolver/). `tests/resolver/run.sh` checks that the copies match and runs the dnsmasq and unbound recipes in a container.

| Resolver | Typical host | Snippet | How it was verified |
| --- | --- | --- | --- |
| dnsmasq | Linux | [resolver/dnsmasq.conf](resolver/dnsmasq.conf) | `tests/resolver/run.sh dnsmasq` (Debian container) |
| unbound | Linux | [resolver/unbound.conf](resolver/unbound.conf) | `tests/resolver/run.sh unbound` (Debian container) |
| `local_unbound` | FreeBSD | [resolver/unbound.conf](resolver/unbound.conf) | CI `freebsd` job |
| systemd-resolved | Linux | [resolver/resolved.conf](resolver/resolved.conf) | CI `linux` job (systemd version recorded below) |
| `/etc/resolver` | macOS | [resolver/macos-resolver](resolver/macos-resolver) | manually (version recorded below) |

## Check emfor first

Before you change any resolver, make sure emfor itself answers:

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A          # Linux, macOS
drill -p 5300 api.svc.emfor @127.0.0.1           # FreeBSD
```

Expect `status: NOERROR`, the `aa` flag, and one A record. `REFUSED` means the name is outside the zone. `NXDOMAIN` means no service has that name. If the daemon logged `service ... is outside DNS zone ...`, that service has to be renamed into the zone before DNS can find it.

## dnsmasq

File: `/etc/dnsmasq.d/emfor.conf`. On Debian and Ubuntu, `conf-dir=/etc/dnsmasq.d` is enabled by default.

```
# Forward the emfor zone to the emfor daemon. Other names keep going upstream.
server=/svc.emfor/127.0.0.1#5300
```

Reload: `systemctl restart dnsmasq`

Check:

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A   # emfor directly
getent hosts api.svc.emfor                # through the system resolver
```

## unbound

File: `/etc/unbound/unbound.conf.d/emfor.conf` on Debian and Ubuntu, where the default config includes that directory. On other systems, add `include: "/path/to/emfor.conf"` to `unbound.conf`.

```
server:
    # emfor listens on loopback; unbound refuses to query localhost by default.
    do-not-query-localhost: no
    # The zone is unsigned under a parent that does not exist in the signed
    # root, so a validating resolver must not try to validate it.
    domain-insecure: "svc.emfor"

forward-zone:
    name: "svc.emfor"
    forward-addr: 127.0.0.1@5300
```

Both `server:` lines are required:
- Without `do-not-query-localhost: no`, unbound never sends the query to 127.0.0.1.
- Without `domain-insecure`, a validating unbound treats every emfor answer as bogus and returns SERVFAIL. This is because the root zone proves that `emfor.` does not exist.

Reload: `unbound-control reload`, or `systemctl restart unbound`

Check:

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A
getent hosts api.svc.emfor
```

### FreeBSD local_unbound

FreeBSD's base system resolver is unbound, set up by `local-unbound-setup`. Its generated config includes `/var/unbound/conf.d/*.conf`.

File: `/var/unbound/conf.d/emfor.conf`, with the same content as the unbound snippet above.

Reload: `service local_unbound restart`

Check:

```sh
drill -p 5300 api.svc.emfor @127.0.0.1   # emfor directly
getent hosts api.svc.emfor                # through the system resolver
```

To run emfor itself, see `examples/rc.d/emfor`.

## systemd-resolved

File: `/etc/systemd/resolved.conf.d/emfor.conf`

```
[Resolve]
DNS=127.0.0.1:5300
Domains=~svc.emfor
```

`~svc.emfor` is a routing-only domain: resolved sends `svc.emfor` names to the DNS server in this file. The `ip:port` form of `DNS=` needs systemd 246 or later. resolved also sends some other names to its global DNS servers, including this one. emfor answers those with REFUSED, and resolved uses the answer from the host's normal servers instead.

Reload: `systemctl restart systemd-resolved`

Check:

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A
resolvectl query api.svc.emfor
getent hosts api.svc.emfor
```

Verified in CI on the GitHub Actions `ubuntu-latest` runner; the systemd version is recorded in the CI log of the `linux` job.

## macOS /etc/resolver

macOS is a development platform only. Create the file `/etc/resolver/svc.emfor`; the file name is the zone:

```
nameserver 127.0.0.1
port 5300
```

No reload is needed. `scutil --dns` should list a resolver for domain `svc.emfor` with port 5300.

Check:

```sh
dig @127.0.0.1 -p 5300 api.svc.emfor A          # emfor directly
dscacheutil -q host -a name api.svc.emfor        # through the system resolver
```

`dig` and `nslookup` bypass `/etc/resolver`, so test the system path with `dscacheutil`, `ping`, or the application itself.

Verified manually on macOS 27.2.1 (build 26B131), 2026-11-12, against a daemon running with the default `[dns]` settings:

```
$ scutil --dns
resolver #8
  domain   : svc.emfor
  nameserver[0] : 127.0.0.1
  port     : 5300
  flags    : Request A records, Request AAAA records

$ dscacheutil -q host -a name api.svc.emfor
name: api.svc.emfor
ip_address: 127.0.0.1
```

`dscacheutil -q host -a name nope.svc.emfor` printed nothing (NXDOMAIN). `ping api.svc.emfor` resolved to the same address.

On macOS the mDNS responder holds UDP 5353, which is why the default port is 5300.

## SRV records

A/AAAA answers carry only an address. When the backends of a service listen on different ports, or a program wants the whole backend list, query SRV instead. emfor answers SRV on two names, with the same records:

- the service name itself: `api.svc.emfor`
- `_<anything>._tcp.<service name>`: `_http._tcp.api.svc.emfor`, the form Go's `net.LookupSRV("http", "tcp", "api.svc.emfor")`, gRPC and similar clients build

`_tcp.api.svc.emfor` exists but has no records (NODATA). `_udp` names are not answered (NXDOMAIN).

An SRV answer lists every backend that may be used: the healthy ones, or all of them when none is healthy and `on_all_down = "open"` (`"closed"` answers SERVFAIL). IPv4 and IPv6 backends are listed together. Each record carries the backend's port, and `balance` sets the priority, weight and order:

| `balance` | priority | weight | order |
| --- | --- | --- | --- |
| `failover` | 0, 1, 2, … in config order (the first healthy backend gets 0) | 1 | config order |
| `weighted` | 0 | the backend's `weight` | config order |
| `locality` | 0 for backends in the host's `site`, 1 for the others (all 0 when none matches) | 1 | rotates on every query |
| `round-robin`, `sticky`, any other value | 0 | 1 | rotates on every query |

SRV clients try the lowest priority first and pick by weight within a priority. Clients that just take the first record still spread out, because the order rotates.

**Target names.** Each record points to a name derived from the backend's IP under the service name, and emfor answers A or AAAA for it, whatever the backend's health:

- IPv4 `10.0.0.11` → `10-0-0-11.api.svc.emfor`
- IPv6 `2001:db8::11` → `2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor` (all eight groups, four digits each)

The addresses also come in the additional section of the SRV answer, so most clients need no second query.

**Size.** Over UDP a reply stays within the size the client advertises with EDNS (at most 1232 bytes), or 512 bytes without EDNS. When the addresses do not fit, emfor leaves them all out, and the client looks up the target names itself. When even the SRV records do not fit, emfor sets the TC flag and the resolver repeats the query over TCP, where the reply is complete. Without EDNS, 512 bytes hold 8 IPv4 backends with their addresses, or 11 without them. With EDNS, 1232 bytes hold 19 and 27.

Check:

```sh
dig @127.0.0.1 -p 5300 _http._tcp.api.svc.emfor SRV   # emfor directly
dig @127.0.0.1 -p 5300 10-0-0-11.api.svc.emfor A      # a target name
dig _http._tcp.api.svc.emfor SRV                      # dnsmasq or unbound on 127.0.0.1:53
resolvectl service _http._tcp api.svc.emfor            # systemd-resolved
```

Expect `status: NOERROR`, the `aa` flag (when asking emfor directly), one SRV record per backend, and the addresses under `ADDITIONAL SECTION`. `tests/resolver/run.sh` checks the dnsmasq and unbound lines, and `tests/resolver/resolved.sh` the systemd-resolved line. On macOS, `dig` bypasses `/etc/resolver`, so test SRV through the application itself.

## Caching

emfor answers with a short TTL (default 1s, per service `dns_ttl`), so a backend that fails drops out of answers within about a second. Resolver caching can stretch that:

- **Minimum cache TTL.** A resolver that raises small TTLs keeps returning a backend after it fails. Leave these at their defaults (0):
  - dnsmasq `min-cache-ttl`
  - unbound `cache-min-ttl`
- **Negative caching.** NXDOMAIN and empty answers are cached for the SOA minimum, which emfor sets to `[dns] ttl`. A name that was not yet configured, or a service with no backend of the asked family, can stay negative for that long after you fix the config.
- **Round-robin and caching.** With `balance = "round-robin"`, emfor rotates on every query it receives. A caching resolver answers repeated lookups from its cache for the TTL, so all programs on the host get the same backend for up to one TTL. Lower `dns_ttl`, down to 0, to rotate more often, at the cost of more queries to emfor.
- **Application caches.** Some runtimes cache DNS results themselves. For example, the JVM uses `networkaddress.cache.ttl`. emfor cannot shorten those.

`on_all_down = "closed"` answers SERVFAIL rather than NXDOMAIN. Resolvers cache SERVFAIL for only a short time (or not at all), so the name works again soon after a backend recovers.
