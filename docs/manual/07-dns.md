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
resolver_timeout 1s;                          # how soon to ask again after a failed lookup

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
`valid=1s` makes it ask again every second. `resolver_timeout 1s` matters
after a lookup has failed, for example when emfor answers `SERVFAIL`: nginx
asks again only after that time, and its default is 30 seconds (below). `service=http` is only the name
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
  nginx's own `proxy_next_upstream` and `max_fails`. **When emfor answers
  again, nginx follows only after `resolver_timeout`**, which is 30 seconds
  by default: in the test, with the default, nginx still had its old list 5
  seconds after emfor answered again and followed after about 26 seconds; with
  `resolver_timeout 1s` it followed after about a second (with 5 seconds
  about 2, with 10 seconds about 7). A change that you make in this state,
  a drain for example, reaches nginx just as late. That is why the recipe
  sets `resolver_timeout 1s`.
- **HTTPS upstreams**: a record's target is a name like
  `10-0-0-11.api.svc.emfor`. Set `proxy_ssl_name` (and
  `proxy_ssl_server_name on`) to the name the backend's certificate has. This
  was not tried.

**TCP, with the `stream` module.** The same `upstream` works in a `stream`
block, for a database or any other TCP service:

```nginx
stream {
    resolver 127.0.0.1:5300 valid=1s ipv6=off;
    resolver_timeout 1s;

    upstream db {
        zone db 64k;
        server db.svc.emfor service=pgsql resolve;
    }

    server {
        listen 5432;
        proxy_pass db;
    }
}
```

`tests/nginx/stream.sh` checked it (with `api.svc.emfor` and `service=http`, not
the names above) with two backends answering HTTP, which a TCP proxy passes
on, with nginx 1.30.6 and 1.31.8. Health, `drain`,
`failover` (the second server is a backup) and the weights behave as for
`http`, and so does `SERVFAIL`, including the `resolver_timeout` after it. What
is particular to TCP:

- **A connection that is already open is not touched.** When its backend was
  drained, a 6 second answer on a connection to it still arrived whole (1200
  of 1200 bytes) while new connections went to the other backend. When the
  backend was stopped instead, the same connection was cut (160 of 1200
  bytes). So after a drain, long connections to the backend end when the client
  or the backend closes them; nothing moves them.
- **A new connection to a stopped backend that emfor still lists goes to the
  next server**: `proxy_next_upstream` is on by default in `stream`, and 0 of
  25 connections made while a backend was stopped failed.

Not tried: UDP (`listen ... udp`), `ssl_preread`, `proxy_protocol`,
`proxy_timeout` (an idle connection is closed after its default of ten
minutes, as nginx documents it), nginx Plus.

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
case. The Integrations workflow runs it every week and when the DNS or
candidate code of emfor changes.


On FreeBSD 15.1 (arm64, nginx 1.30.6 from the packages) the same checks
passed, including the older nginx case. The recipes need no change; the
nginx package reads `/usr/local/etc/nginx/nginx.conf`, not `/etc/nginx`.
The Docker test itself does not run there; the script
(`tests/nginx/inside.sh`) was run by hand, and it needed three things the
Linux container has: `dig` (a wrapper around `drill` did), the address
`127.0.0.2` on the loopback (`ifconfig lo0 alias 127.0.0.2/32`; FreeBSD's
loopback has only `127.0.0.1`, and the older-nginx check binds a backend
to `127.0.0.2`), and `nginx` reading `/etc/nginx/nginx.conf`. The
`stream` checks (`tests/nginx/stream.sh`) passed there too, by hand, with
one more line: the FreeBSD package builds `stream` as a dynamic module, so
`nginx.conf` needs `load_module /usr/local/libexec/nginx/ngx_stream_module.so;`
at its top (without it `nginx -t` says `unknown directive "stream"`). The
delay after a `SERVFAIL` was the same there: about 25 seconds with the
default `resolver_timeout`, and about a second with `resolver_timeout 1s`.

## Using it with HAProxy

HAProxy can fill the servers of a backend from SRV answers: a
`server-template` names the SRV query and a number of slots, and HAProxy
puts the records into the slots, asking again when the answer changes. With
emfor's DNS server as its resolver, a backend that fails emfor's checks, one
you drain and one you add all reach HAProxy within a few seconds, without a
reload:

```
resolvers emfor
    nameserver emfor 127.0.0.1:5300
    accepted_payload_size 4096
    hold valid 1s
    resolve_retries 3
    timeout resolve 1s
    timeout retry 1s

backend api
    balance roundrobin
    server-template s 1-4 _http._tcp.api.svc.emfor resolvers emfor resolve-opts allow-dup-ip resolve-prefer ipv4 init-addr none
```

- `nameserver` is emfor's DNS server (`[dns] listen`, port 5300 by default),
  not the host resolver.
- `accepted_payload_size 4096` lets the answer of a service with many
  backends arrive over UDP (HAProxy asks for 512 bytes by default).
- `hold valid 1s` makes HAProxy use a new answer after a second; the other
  `hold` values stay at their defaults, which matter below.
- `1-4` is the number of **slots**: the most backends HAProxy can use. A
  service with more backends than slots is cut off (the test: three backends
  in four slots use three). Make it larger than the service.
- `_http._tcp.api.svc.emfor` is the SRV name; emfor answers
  `_<anything>._tcp.<service>` ([SRV records](#srv-records)), and the port
  comes from each record. `allow-dup-ip` is needed when several backends
  share one address with different ports, and `init-addr none` lets HAProxy
  start before the first answer.
- No `check` is needed: a backend that fails emfor's checks leaves the
  answer. You can add HAProxy's own `check` as a second opinion.

What HAProxy does with the answer, as checked with HAProxy 3.4.7 and 2.8.31:

- **Health and `drain = true`** ([chapter
  5](05-backend-selection.md#draining-a-backend)) reach HAProxy within a few
  seconds, as for nginx: the backend gets nothing, and gets requests again
  when it recovers or the drain is removed. HAProxy is not reloaded.
- **Priorities are not used.** With `balance = "failover"`, HAProxy sent
  requests to both backends (20 and 20 of 40), not only to the first one.
  The same holds for `locality` and `fastest`, which rank backends by
  priority 0 and 1. Do not rely on a backup in the SRV answer; use
  HAProxy's own `backup` for that, or the nginx recipe above, which does
  follow priorities.
- **Weights are not used.** With `balance = "weighted"` and weights 3 and 1,
  the requests went 40 and 40 of 80, also with `weight 1` or
  `ignore-weight` on the template.
- **When emfor does not answer** (stopped, or restarting): HAProxy keeps its
  servers for the `hold` times. With the settings above it still served all
  requests 4 seconds after emfor stopped.
- **When emfor answers `SERVFAIL`** (no backend to offer, or every backend
  drained): HAProxy kept its servers for a while and then dropped them, and
  every request got `503`; in the test it had lost them between 10 and 30
  seconds later, which is the default `hold other` (30 s). nginx, in
  the same case, keeps its last list for good.

Which is right depends on what you want to happen when emfor says there is
no backend. The `hold` values are HAProxy's, and were not tuned here.

Three variations, as `tests/haproxy/more.sh` saw them with HAProxy 3.4.7 and
2.8.31:

- **`mode tcp`** (in `defaults` or in the frontend and the backend) changes
  nothing: the servers come from the same answers, a backend that fails its
  checks or is drained gets no connections, and it gets them again when it
  recovers.
- **TLS to the backends.** Add `ssl` to the template. With `ssl verify none`
  it works. To check the certificate, `ssl verify required ca-file <ca> sni
  str(<name>)` checks it against `<name>`: with the right name both
  backends were served, with a name that is not in the certificate nothing
  was. Always set `sni`: the names of the SRV targets
  (`10-0-0-11.api.svc.emfor`) are not in a certificate, and without `sni`
  3.4.7 refused the certificate while 2.8.31 accepted any certificate that
  the CA had signed, whatever its name. Do not rely on `verifyhost`: a wrong
  one next to a right `sni` was served by both, and a right one without
  `sni` was served by 2.8.31 and refused by 3.4.7.
- **One `nameserver`.** Use one `nameserver` line, the emfor server on the
  same host as HAProxy. HAProxy sends every query to every nameserver and
  uses the first reply that arrives; it does not merge them (in the test, with
  two emfor servers that gave the same answer, HAProxy kept serving with
  either one stopped, and a backend that went down still left the list). When
  the two answers differ (a backend drained on one only), the list changes
  from query to query: HAProxy removes a backend and adds it again, often in
  another slot, and what it serves differs from run to run with the same
  configuration (many runs on Linux with 3.4.7 and 2.8.31, and 6 on FreeBSD
  with 3.4.4). Requests can also fail: after such a change a slot was ready
  with the right address and port 0, and a request sent to it got a `503`
  after about 3 seconds (a backend stopped and started every 1.5 seconds:
  with two nameservers 5 of 5 runs on Linux 3.4.7, and with one nameserver
  at most one such request per run; but on FreeBSD the same test gave them
  with one nameserver too: 29 to 45 of 80 requests in each of 5 runs with
  3.4.4 (the package), and 21 to 28 with 3.4.7 (built from the port), so it is
  not a change between those releases, and the second nameserver is not the
  only cause; why FreeBSD shows it and the Linux container does not was not
  found). These were seen by hand, not in the test, and
  the exact cause inside HAProxy was not found. For availability run an emfor server next to each HAProxy
  rather than giving one HAProxy several nameservers. If you give it two
  anyway, they must agree, `drain` included; `option redispatch` hides the
  failed requests but does not repair the slot. `hold obsolete 30s` makes
  the result stable by keeping the union of the answers, so a drain or a
  removal takes that long to count: not recommended.

[tests/haproxy/](../../tests/haproxy/run.sh) runs these checks on a given
HAProxy image (`tests/haproxy/run.sh haproxy:2.8`), including the two things
HAProxy does not do, so that the test fails if a later HAProxy starts to
follow priorities or weights and this section must change. The Integrations
workflow runs it every week and when the DNS or candidate code of emfor
changes. Not tried: the TLS checks of emfor itself next to HAProxy's, client
certificates to the backends, `resolvers` with more than two name servers,
other versions than the two above.


On FreeBSD 15.1 (arm64, HAProxy 3.4.4 from the packages) the same checks
passed, priorities and weights not followed included, so the section holds
there as it is. As with nginx, `tests/haproxy/inside.sh` was run by hand;
it needed nothing else than the HAProxy package. `tests/haproxy/more.sh`
(`mode tcp`, TLS to the backends, two name servers) passed there too, by
hand, with the same results as with 3.4.7, the `sni` ones included. Not tried
on FreeBSD: other HAProxy versions.

## Using it with Caddy

Caddy's `reverse_proxy` can take its upstreams from SRV answers with
`dynamic srv`, so health and drain reach Caddy within a few seconds without a
reload:

```
:80 {
    reverse_proxy {
        dynamic srv {
            name _http._tcp.api.svc.emfor
            refresh 1s
            resolvers 127.0.0.1:5300
            grace_period 30s
        }
        transport http {
            resolvers 127.0.0.1:5300
        }
        lb_try_duration 2s
        lb_try_interval 100ms
    }
}
```

- **Two `resolvers`.** The one in `dynamic srv` is for the SRV query. The
  targets of the answer are names such as `10-0-0-11.api.svc.emfor`, which
  Caddy dials, and the one in `transport http` is for that. Without it Caddy
  asks the host resolver, and when that does not know the zone every request
  fails (10 of 10 in the test, `no such host` in the log); with a host
  resolver that [forwards the
  zone](#forwarding-from-the-host-resolver) it is not needed.
- **`refresh`** is how often Caddy asks again; the test used 1 second. The
  default is longer (a minute, as I read the Caddy documentation; not
  tried), and a longer `refresh` delays a drain or a failure that long.
- **`grace_period`** keeps the last list when the lookup fails. emfor
  answers `SERVFAIL` when every backend of the service is down or drained:
  with `grace_period` Caddy kept answering (10 of 10), without it Caddy had no
  upstream and every request failed (10 of 10). nginx keeps its last list
  by itself.
- **`lb_try_duration` and `lb_try_interval`** make Caddy try again, so a
  backend that has just stopped, and is still in the list for the second or
  so that emfor and Caddy need to notice it, loses nothing (0 of 25 requests
  in the test). Without them Caddy does not retry, and 3 of 25 were lost.

What Caddy does with the answer, as `tests/caddy/run.sh` saw it with Caddy
2.11.9:

- A backend that fails its checks leaves the list, and one that recovers
  returns; a `drain = true` backend leaves it too, without a reload of
  Caddy and with no request lost (with the retries above).
- **Weights are not followed**: a 3 to 1 service gave about half each (41 and 39 of 80 in one run).
- **Priorities are not followed by the default policy**: a `failover` service
  sent requests to the backup as well. With `lb_policy first` Caddy takes the
  backends in the order of the answer, which is sorted by priority, so
  `balance = "failover"` with `lb_policy first` sends everything to the
  first backend while it is healthy, to the second when it is not, and back
  when it returns.

[tests/caddy/](../../tests/caddy/run.sh) runs these checks on a given Caddy
image (`tests/caddy/run.sh caddy:2`); the Integrations workflow runs it every
week and when the DNS or candidate code of emfor changes. Not tried: HTTPS
upstreams, Caddy's own active health checks next to emfor's, load balancing
policies other than `first` and the default, several Caddy instances, other
versions than 2.11.9, FreeBSD.

## Using it with Prometheus

Prometheus can find its scrape targets from SRV answers with
`dns_sd_configs` of type `SRV`: the targets are the backends of an emfor
service that are healthy, and the list follows the checks without a reload
of Prometheus.

```yaml
scrape_configs:
  - job_name: api
    dns_sd_configs:
      - names: ["_http._tcp.api.svc.emfor"]
        type: SRV
        refresh_interval: 5s
```

`dns_sd_configs` asks the host's resolver and has no setting for a name
server or a port (`promtool check config` rejects a `server` field), so
the host resolver must forward the zone to emfor, as in [Forwarding from the
host resolver](#forwarding-from-the-host-resolver). It must forward the whole
zone, not only the SRV query: the targets of the answer are names such as
`10-0-0-11.api.svc.emfor`, which Prometheus resolves again for each scrape
and which emfor answers. The `instance` label is that name and the port, not
the address; `__meta_dns_srv_record_target` and `__meta_dns_srv_record_port`
are there for `relabel_configs`. The port of an SRV record is the port of the
backend in emfor, so the application's metrics have to be on it; for another
port, make a second service whose backends are that port.

What the targets are, as `tests/prometheus/run.sh` saw it with Prometheus
3.16.0 (`refresh_interval` 1 second, emfor as the only resolver of the
container):

- Both backends of a `round-robin` service are targets and `up`.
- A backend that fails its checks **is not a target any more**: it is not
  shown as down, and its `up` series ends. So `up == 0` never fires for it;
  alert on emfor's own metrics instead (`emfor_backend_state`, see
  [examples/prometheus/](../../examples/prometheus/emfor-rules.yml)).
- A drained backend is not a target either, though it runs and answers on
  `/metrics`: during a [deploy with drain](08-operations.md#rolling-deploy-with-drain)
  its metrics are not collected.
- A `failover` service lists its backup too (priority 1), and Prometheus
  takes every record, so it scrapes both. A backend with `backup = true`
  is a target only while no primary is healthy.
- A target appears and disappears within about `refresh_interval` plus the
  time emfor needs to see the change (`interval`, `fall`, `rise`).

So an emfor service is a good source for "the backends that are serving
now". To scrape every backend whatever its health, and to see `up == 0`,
keep a static list or file-based discovery for the scrape and use emfor for
the alerts.

[tests/prometheus/](../../tests/prometheus/run.sh) runs these checks (the
Integrations workflow runs it every week and when the DNS or candidate
code of emfor changes). On FreeBSD 15.1 the same script passed by hand with
the Prometheus 2.55.1 package (nginx 1.30.6 from the packages; `local_unbound`
stopped, emfor on port 53). Not tried: dnsmasq or unbound in front of emfor
(the tests used emfor as the only resolver), other Prometheus versions, HTTPS
targets, many targets, the default `refresh_interval` of 30 seconds.

## Using it with pf (FreeBSD)

On FreeBSD, pf can spread connections over a table of addresses without a
proxy in the data path: an `rdr` rule redirects the connections that come to
an address to the members of a table, in turn. emfor has no hook that runs a
command when its answer changes, so a small loop asks emfor's DNS server for
the SRV records of the service every second and makes the table equal to what
it offers:

```
table <api> persist
rdr pass on $ext_if inet proto tcp from any to $vip port 80 -> <api> port 8000 round-robin
```

[examples/pf/emfor-pf-table.sh](../../examples/pf/emfor-pf-table.sh) is that
loop (`emfor-pf-table.sh loop api.svc.emfor api`, as root; for example under
`daemon(8)`, not tried). It takes the SRV records with the lowest priority,
reads the address out of each target name (`10-0-0-11.api.svc.emfor` is
`10.0.0.11`) and runs `pfctl -t api -T replace` when the set changed. It needs
`drill`, which the base system has. All backends of the service must have the
same port, the one in the `rdr` rule.

What happens, as `tests/pf/run.sh` saw it on FreeBSD 15.1, with the backends
and the redirected address on the loopback:

- **Health**: a backend that fails its checks leaves the table, and returns
  when it recovers. **`drain = true`** removes it the same way
  ([chapter 5](05-backend-selection.md#draining-a-backend)).
- **A connection that is already open is not cut** when its backend leaves the
  table, because pf keeps its state: a 6 second answer on a connection to a
  drained backend arrived whole (1200 of 1200 bytes), while new connections
  went to the other backend.
- **Priorities**: only the lowest priority is in the table, so a `failover`
  service has its first backend there, and the second when the first is
  down. A `backup = true` backend is out while a primary is healthy.
- **Weights are not used**: pf's `round-robin` shares equally (40 and 40 of 80
  for weights 3 and 1).
- **When emfor has nothing to offer** (`SERVFAIL`: every backend drained, or
  down with `on_all_down = "closed"`) the table is emptied, and pf drops a
  connection that an empty `rdr` table cannot redirect: the client waits for
  its own timeout (seen from the loopback and from another host). Set `EMPTY`
  to an address on the host where nothing listens on the `rdr` port, and the
  table holds that instead of nothing; a client on another host was refused at
  once by a table entry where nothing listened (`rc=7` in a second). The
  script's `EMPTY` itself was only checked for what it puts in the table.
- **When emfor does not answer** (stopped, not reachable) the table is left as
  it is, and follows emfor again when it is back.
- Between the moment a backend fails and the next pass of the loop (a second,
  after emfor's own checks), pf still sends new connections to it: from another
  host a connection to a backend where nothing listened was refused, so the
  client should retry.

Not tried: IPv6 (the script reads IPv6 targets but they were not run),
`source-hash` and `sticky-address`, `rdr` on a real interface with many
clients (the checks of other hosts were made by hand from one machine: refused
and dropped connections), `daemon(8)` or an rc script for the loop, other
firewalls. `tests/pf/run.sh` needs root and a FreeBSD with the pf module; it
loads its own rules and enables pf, so it refuses to run when pf is already
on. It is not part of the CI.

## Clients that ask for SRV records themselves

Some clients find their servers with SRV records on their own, so no proxy is
needed: they ask `_<service>._tcp.<name>`, and emfor answers
`_<anything>._tcp.<service>` with the healthy backends, their ports, and the
priorities and weights of [SRV records](#srv-records). A server that fails
emfor's checks, or is drained, is not in the answer. They ask the host's
resolver, so it must forward the zone to emfor
([Forwarding from the host resolver](#forwarding-from-the-host-resolver)); the
test below made emfor the only resolver of the container, on port 53. Only
`_tcp` names are answered: a `_udp` name gets `NXDOMAIN`.

- **Kerberos (MIT).** With `dns_lookup_kdc = true` in `krb5.conf` and the
  realm equal to the service name in capitals (`KDC.SVC.EMFOR` for the service
  `kdc.svc.emfor`), `kinit` asked `_kerberos._udp.KDC.SVC.EMFOR` (no answer, as
  said), then `_kerberos._tcp.KDC.SVC.EMFOR`, and went to both KDCs over TCP. A KDC
  that failed its checks was not offered, and `kinit` went to the other only.
  With `balance = "failover"` the answer has priorities 0 and 1 and `kinit` tried
  the primary first. emfor's `tcp` check looks at the TCP port only.
- **OpenLDAP.** `ldapsearch -H 'ldap:///dc=ldap%2Cdc=svc%2Cdc=emfor'` (no host,
  a DN) makes libldap ask `_ldap._tcp.ldap.svc.emfor` and use the servers of
  the answer. **Write the commas of the DN as `%2C`**: with plain commas
  `ldapsearch` says `Could not parse LDAP URI(s)`. A server that failed its
  checks was not used (3 of 3 attempts went to the other).
- **MongoDB.** `mongodb+srv://db.svc.emfor/` gave PyMongo the two servers of
  `_mongodb._tcp.db.svc.emfor`. emfor has no TXT records and answers the TXT
  query that `mongodb+srv` also makes with `NOERROR` and no records, which
  PyMongo accepted. `mongodb+srv` turns TLS on: add `tls=false` to the URI when
  the servers do not speak TLS. A server that failed its checks, and one that
  was drained, was not in the list.

[tests/srv/](../../tests/srv/run.sh) checks these with the Debian bookworm
packages `krb5-user` (MIT Kerberos), `ldap-utils` (OpenLDAP 2.5.13) and
`python3-pymongo` (3.11.0); the servers are plain listeners, so what is
checked is which servers the client is told about and goes to. Not tried: a
real KDC, LDAP or MongoDB server behind them (`kinit` ended with `Cannot contact
any KDC`), LDAPS, sssd (`ldap_uri = _srv_`), nslcd, Samba, Heimdal, Active
Directory names such as `_msdcs`, a realm or domain outside `svc.emfor` (the
`[dns] zone` of its own that it would need), other drivers than PyMongo, other
versions. The Integrations workflow runs the test every week and when the DNS
code of emfor or the test changes.

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
