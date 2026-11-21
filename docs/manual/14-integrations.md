# 14. Integrations

[Contents](README.md) · Previous: [13. Security model](13-security.md) · Next: [15. Upgrading](15-upgrading.md)

How to make proxies, monitoring and firewalls follow emfor. Each recipe
lists the test in the repository that exercises it and the versions it names.
**None of these tests was rerun for this manual**; the results below are those
the repository's tests and the previous manual report.

All proxy recipes use emfor's DNS server with SRV records
([chapter 10](10-dns.md#srv-records)), so they need `[dns]` in emfor's config.
They have the same limits:

- a proxy sees a change after emfor's checks notice it and the proxy asks
  again (about a second with the settings below);
- when emfor is stopped, DNS gives no answer, and each proxy keeps its last
  list for a while;
- when a service has nothing to offer (every backend drained, or all down
  with `on_all_down = "closed"`), emfor answers SERVFAIL.

## nginx

Open-source nginx 1.27.3 or later can take an upstream from SRV answers:

```nginx
resolver 127.0.0.1:5300 valid=1s ipv6=off;    # emfor's DNS server, asked every second
resolver_timeout 1s;                          # ask again soon after a failed lookup

upstream api {
    zone api 64k;                              # required with `resolve`
    server api.svc.emfor service=http resolve; # asks _http._tcp.api.svc.emfor
}

server {
    listen 80;
    location / { proxy_pass http://api; }
}
```

| | nginx |
| --- | --- |
| Health and drain | Followed within seconds, no reload. Open connections stay open. |
| Priorities (`failover`) | Followed: priority 0 are servers, others `backup`. |
| Weights (`weighted`) | Followed (60 of 80 requests for weights 3:1). |
| SERVFAIL | Keeps its last list; after emfor answers again it follows after `resolver_timeout` (default 30 s, hence the `1s` above). |
| The `stream` module | The same `upstream` works for TCP. |
| Tested by | `tests/nginx/run.sh`, `tests/nginx/stream.sh`; nginx 1.30.6 and 1.31.8. |

Point `resolver` at emfor directly, not at the host resolver.

## HAProxy

```text
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

| | HAProxy |
| --- | --- |
| Health and drain | Followed within seconds, no reload. |
| Priorities, weights | **Not used**: `failover` sends to both backends, weights 3:1 give 40:40. Use HAProxy's own `backup` and `weight`. |
| Slots | `1-4` is the most backends it can use; make it larger than the service. |
| emfor stopped | Keeps its servers for the `hold` times. |
| SERVFAIL | Drops the servers after `hold other` (30 s): every request gets 503. |
| TLS to backends | Add `ssl ... sni str(<name>)`; always set `sni`, since the SRV target names are not in a certificate. |
| Name servers | Use one: the emfor on the same host. Two that disagree make the server list change from query to query. |
| Tested by | `tests/haproxy/run.sh`, `tests/haproxy/more.sh`; HAProxy 3.4.7 and 2.8.31, and 3.4.4 on FreeBSD 15.1. |

## Caddy

```text
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

The second `resolvers` (under `transport http`) is for the SRV target names
such as `10-0-0-11.api.svc.emfor`, which Caddy dials; without it, Caddy asks
the host resolver, which must then forward the zone. `grace_period` keeps the
last list when a lookup fails. Tested by `tests/caddy/run.sh` with Caddy
2.11.9.

## Prometheus

### Scraping the backends emfor finds

```yaml
scrape_configs:
  - job_name: api
    dns_sd_configs:
      - names: ["_http._tcp.api.svc.emfor"]
        type: SRV
        refresh_interval: 5s
```

`dns_sd_configs` uses the host resolver, which must forward the whole zone
([chapter 10](10-dns.md#forwarding-from-the-host-resolver)). A backend that
fails its checks, or is drained, **stops being a target**: its `up` series
ends instead of going to 0. Use this for "the backends serving now", and keep
a static list to see `up == 0`. Tested by `tests/prometheus/run.sh` with
Prometheus 3.16.0.

### Scraping emfor itself

```yaml
scrape_configs:
  - job_name: emfor
    static_configs:
      - targets: ["127.0.0.1:5301"]
```

[examples/prometheus/emfor-rules.yml](../../examples/prometheus/emfor-rules.yml)
has alerting rules on emfor's metrics: `EmforMetricsDown`,
`EmforServiceAllDown`, `EmforBackendUnhealthy`, `EmforBackendFlapping`,
`EmforBackendCheckErrors`, `EmforBackendNotChecked` and
`EmforStoreCommitFailing`. There is no rule for the store filling up, plugins
or peers; add your own on `emfor_store_used_bytes`, `emfor_plugin_up` and
`emfor_peer_up` if you need them. The rules come with
`promtool` unit tests in `emfor-rules.test.yml`:

```sh
$ promtool check rules examples/prometheus/emfor-rules.yml
$ promtool test rules examples/prometheus/emfor-rules.test.yml
```

## pf (FreeBSD)

pf can spread connections over a table with an `rdr` rule;
[examples/pf/emfor-pf-table.sh](../../examples/pf/emfor-pf-table.sh) keeps the
table equal to emfor's SRV answer of lowest priority, once a second:

```text
table <api> persist
rdr pass on $ext_if inet proto tcp from any to $vip port 80 -> <api> port 8000 round-robin
```

```sh
# emfor-pf-table.sh loop api.svc.emfor api
```

- Every backend must use the port of the `rdr` rule.
- Open connections are not cut when a backend leaves the table.
- Weights are not used; `failover` puts only the first backend in.
- On SERVFAIL the table is emptied, and pf drops connections it cannot
  redirect; set `EMPTY` to a local address where nothing listens to make them
  fail fast. When emfor does not answer, the table stays as it is.
- Tested by `tests/pf/run.sh` on FreeBSD 15.1 (by hand, not in CI). IPv6 was
  not run.

## Nagios, Icinga and other monitoring

### Watching emfor: check_emfor.sh

[examples/nagios/check_emfor.sh](../../examples/nagios/check_emfor.sh) reads
`emfor status --json` (it needs `jq`) and reports in the Nagios plugin
convention:

```console
$ check_emfor.sh -s web.svc.emfor
EMFOR WARNING - web.svc.emfor: 2 of 3 healthy (127.0.0.1:7002 WARNING - slow on 7002) | 'web.svc.emfor_healthy'=2;3;1;0;3
```

| Status | When |
| --- | --- |
| CRITICAL (2) | The daemon is stale or stopped, or fewer than `-c` (default 1) backends are healthy. |
| WARNING (1) | Fewer than `-w` (default: every backend that is not drained) are healthy. |
| UNKNOWN (3) | `emfor` or `jq` cannot run, the store cannot be read, or no such service. |

Drained backends are not counted. The monitoring user needs the same store
access as an application ([chapter 12](12-operating.md#who-can-read-the-store)).

### Using a Nagios plugin as a check

An `exec` check passes the backend in `EMFOR_IP` and `EMFOR_PORT`, not as
arguments. [examples/nagios/emfor-exec.sh](../../examples/nagios/emfor-exec.sh)
turns them into `-H` and `-p` and runs the plugin:

```toml
[services."web.svc.emfor"]
check = "exec"
backends = ["10.0.0.11:80", "10.0.0.12:80"]
interval = "10s"
timeout = "5s"
check_options = { command = ["/usr/local/libexec/emfor-exec.sh", "/usr/local/libexec/nagios/check_http", "-u", "/healthz"] }
```

The plugin's exit status becomes emfor's result: 0 up, 1 (WARNING) and 2
(CRITICAL) down, 3 (UNKNOWN) and others error; the first output line is the
reason. `--warning up` makes WARNING count as up; `--host-flag` and
`--port-flag` change the flags. Both scripts are tested by
`tests/nagios/run.sh` with a stand-in plugin; real Monitoring Plugins, Nagios,
Icinga and Zabbix were not tried.

## Programs that ask for SRV records

Go's `net.LookupSRV("http", "tcp", "api.svc.emfor")` asks
`_http._tcp.api.svc.emfor`, which emfor answers. Such a client gets every
candidate with its port and should try them in priority order. A client that
takes only the first record still spreads out, because the order rotates.
