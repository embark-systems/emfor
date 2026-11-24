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
| Name servers | Use one: the emfor on the same host. Two that disagree make the server list change from query to query; when several backends share one IP (one host, several ports), HAProxy 3.4 then also gave 503 for a few requests (a slot with port 0; 6 of 6 runs of 400 requests, and none in 6 runs with a separate IP per backend). One name server is not enough there: with one backend stopped and started every 1.5 s, 7 runs of 100 requests with a shared IP gave 0 to 12 requests of 503 after about 3 s each (6 runs had some); with a separate IP per backend the only failure was 1 request in each of 3 runs, while the backend was down. On FreeBSD 15.1 (HAProxy 3.4.4) the same test with a shared IP gave 14 to 38 requests of 503 in each of 4 runs of 100, and 1 to 2 with a separate IP per backend in each of 4 runs. Sampling `show servers state` during one run showed where the 503s go: slots with no address and port 0 (`s4` always, `s2` while its backend was down) stay in state running, and the log shows `SC--` for them. |
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

## Old network management systems: SNMP and hooks

emfor has no SNMP agent or trap sender, and no command that runs when a state
changes. An enterprise OID has to come from IANA, a MIB has to be kept, and
SNMPv3 needs MD5, SHA and AES, which does not fit what [chapter
13](13-security.md#fips) says about the algorithms emfor uses. A hook has to be
told what to do when the command hangs, fails, or runs thousands of times for
one flapping backend. What emfor gives you instead is a stable record of every
change, which a small program can turn into whatever the other system wants.

**Poll `emfor status --json`.** Each backend has its last 20 state changes in
`history` ([chapter 12](12-operating.md#emfor-status)). [examples/snmp/emfor-snmp-trap.sh](../../examples/snmp/emfor-snmp-trap.sh)
remembers the newest change it sent and sends one SNMPv2c trap, with
`snmptrap` from net-snmp, for each newer one. Run it from cron or a systemd
timer, more often than 20 changes can happen:

```console
$ emfor-snmp-trap.sh -H nms.example.net -o 1.3.6.1.4.1.99999.1 -c secret
```

`1.3.6.1.4.1.99999` stands for **your** enterprise number; the trap is `OID.1`
and carries the service, backend, old state, new state and reason as strings
(`OID.2` to `OID.6`). Write the MIB for these yourself. The first run sends
nothing and only records where it is. The manual ran it against net-snmp
5.9.3's `snmptrapd` in a Debian 12 container, with a backend stopped and
started again: one trap for each change, none again on the next run. It has
not sent a trap to a management station of another make. After the daemon
restarts, every backend goes from `unknown` to `healthy` and the script sends
a trap for each of those changes; a receiver has to expect that burst. A
community string is sent in clear text; for SNMPv3 give `snmptrap`
its own options, for example through `snmp.conf`, and change the script's
arguments to match.

**Read the log.** With `[log] format = "json"` ([chapter
12](12-operating.md#json-lines)) a state change is a line with `event` set to
`backend_state_changed`. rsyslog's `omprog`, or a program that reads the journal
(`journalctl -u emfor -o cat -f`), can act on each line as it comes, with no
polling delay. Lines before the config is read are text, so the program has to
skip lines that are not JSON.

**Let the management station ask.** net-snmp's `snmpd` can expose a command's
output with `extend`; the station then reads it with SNMP GET:

```text
extend emfor-web /usr/local/bin/check_emfor.sh -s web.svc.emfor
```

The result and the output of the command are in `NET-SNMP-EXTEND-MIB`
(`nsExtendResult` and `nsExtendOutput1Line`). `snmpd` runs it as its own user,
which needs the same store access as an application
([chapter 12](12-operating.md#who-can-read-the-store)). The line was run
against `snmpd` 5.9.3 in a Debian 12 container (as root): `nsExtendResult` was
0 with every backend healthy and 2 (CRITICAL) with the only backend stopped.
`snmpd` keeps the output for 5 seconds (`nsExtendCacheTime`), so a change shows
up after that. A plain Debian install has no `NET-SNMP-EXTEND-MIB` file for
names like these (it is in `snmp-mibs-downloader`, in non-free); read the
numeric OIDs `.1.3.6.1.4.1.8072.1.3.2.3.1.4` (result) and `.3.1.1` (first
line of output), followed by the length and the characters of the name.

## Programs that ask for SRV records

Go's `net.LookupSRV("http", "tcp", "api.svc.emfor")` asks
`_http._tcp.api.svc.emfor`, which emfor answers. Such a client gets every
candidate with its port and should try them in priority order. A client that
takes only the first record still spreads out, because the order rotates.
