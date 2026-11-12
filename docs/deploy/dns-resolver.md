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

Verified manually on: not yet verified.

## Caching

emfor answers with a short TTL (default 1s, per service `dns_ttl`), so a backend that fails drops out of answers within about a second. Resolver caching can stretch that:

- **Minimum cache TTL.** A resolver that raises small TTLs keeps returning a backend after it fails. Leave these at their defaults (0):
  - dnsmasq `min-cache-ttl`
  - unbound `cache-min-ttl`
- **Negative caching.** NXDOMAIN and empty answers are cached for the SOA minimum, which emfor sets to `[dns] ttl`. A name that was not yet configured, or a service with no backend of the asked family, can stay negative for that long after you fix the config.
- **Round-robin and caching.** With `balance = "round-robin"`, emfor rotates on every query it receives. A caching resolver answers repeated lookups from its cache for the TTL, so all programs on the host get the same backend for up to one TTL. Lower `dns_ttl`, down to 0, to rotate more often, at the cost of more queries to emfor.
- **Application caches.** Some runtimes cache DNS results themselves. For example, the JVM uses `networkaddress.cache.ttl`. emfor cannot shorten those.

`on_all_down = "closed"` answers SERVFAIL rather than NXDOMAIN. Resolvers cache SERVFAIL for only a short time (or not at all), so the name works again soon after a backend recovers.
