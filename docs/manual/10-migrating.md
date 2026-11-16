# 10. Moving from HAProxy or Consul

[Manual](README.md) · Previous: [9. Troubleshooting](09-troubleshooting.md)

This chapter is for people who already run HAProxy or Consul to keep
applications away from broken backends, and want to see what the same job
looks like in emfor. It translates a real-looking configuration of each
into `emfor.toml`, says what has no equivalent, and ends with a way to try
emfor next to what you have, without touching traffic.

Read [chapter 1](01-concepts.md) first: the picture is different enough
that a line-by-line translation alone will mislead you.

## Is emfor the right replacement?

emfor does one thing of what HAProxy and Consul do: it checks a fixed list of
backends from the host of the application and tells the application which
one to use. It is not in the path of the traffic, and it has no central
server.

| | HAProxy | Consul | emfor |
| --- | --- | --- | --- |
| Where the traffic goes | Through HAProxy (one more hop) | Straight to the backend | Straight to the backend |
| Who decides which backend | HAProxy, for every connection | The application, from a DNS answer | The application, from the local store (client library or `emfor lookup`) or from a DNS answer |
| Where the checks run | In HAProxy | In the Consul agent of the node that runs the service | On every application host, against every backend |
| Which backends exist | The config file | Services register themselves | The config file of each host |
| A central server | No | Yes (servers with a quorum) | No |

So emfor fits when the backends are a known list of addresses (virtual
machines, bare metal, databases, AAA servers), and the application can ask
emfor where to connect, either with the client library, with a shell
command, or through DNS.

It does not replace what a proxy does with the traffic itself: least
connections, retries, queueing, TLS termination, routing by URL or header,
connection limits, and passive health checks (marking a server down because
real requests failed). If you need those, keep HAProxy and see
[what does not carry over](#what-does-not-carry-over).

## From HAProxy

An HAProxy backend like this one:

```
backend api
    balance roundrobin
    option httpchk GET /healthz
    http-check expect status 200
    default-server inter 2s fall 3 rise 2
    server api1 10.0.0.11:8080 check
    server api2 10.0.0.12:8080 check weight 3
    server api3 10.0.1.11:8080 check backup slowstart 30s
```

becomes this service in `emfor.toml`:

```toml
[services."api.svc.emfor"]
check = "http"
balance = "weighted"
interval = "2s"
fall = 3
rise = 2
slow_start = "30s"
backends = [
  "10.0.0.11:8080",
  { addr = "10.0.0.12:8080", weight = 3 },
  { addr = "10.0.1.11:8080", backup = true },
]

[services."api.svc.emfor".check_options]
path = "/healthz"
expect_status = [200]
```

Line by line:

| HAProxy | emfor | Notes |
| --- | --- | --- |
| `backend api` | `[services."api.svc.emfor"]` | The service name is host-name-like and is also the DNS name ([chapter 3](03-configuration.md#servicesname)). |
| `server api1 10.0.0.11:8080` | an entry of `backends` | IP addresses only, never host names: emfor does not depend on DNS to find its backends. |
| `check` | `check = "http"` (or `"tcp"`) | One `check` for the whole service. |
| `inter 2s` | `interval = "2s"` | Both default to 2 seconds. |
| `fall 3` / `rise 2` | `fall = 3` / `rise = 2` | Both default to 3 and 2. |
| `option httpchk GET /healthz` | `path = "/healthz"` | emfor always sends `GET`. HAProxy without arguments sends `OPTIONS /`; emfor sends `GET /` unless you set `path` ([http check](04-health-checks.md#http)). |
| `http-check expect status 200` | `expect_status = [200]` | Without it, any status from 200 to 399 is `up`. HAProxy's default accepts 2xx and 3xx too. Ranges are written `"200-299"`. |
| `http-check send ... hdr Host x` | `host_header = "x"` | Only the `Host` header can be set. |
| `weight 3` | `{ addr = "...", weight = 3 }` | From 1 to 100, and only used by `balance = "weighted"` (and by `sticky` with a key). HAProxy weights go to 256, and 0 takes a server out: emfor refuses `weight = 0` and `weight = 256` (`weight must be from 1 to 100`), so rescale the weights to 1 to 100 and use `drain = true` for 0 (see the table at the end). |
| `backup` | `backup = true` | A backup is used only while no other backend passes its checks ([backup backends](05-backend-selection.md#backup-backends)). |
| `slowstart 30s` | `slow_start = "30s"` | A recovered backend gets a growing share of the traffic, in ten steps ([slow start](05-backend-selection.md#slow-start)). Set on the service, not on a backend. |
| `balance roundrobin` | `balance = "round-robin"` or `"weighted"` | HAProxy's round robin takes the weights into account; emfor's `round-robin` does not. Use `weighted` when you use weights. |
| `balance first` | `balance = "failover"` | The first backend that is healthy, in config order. Without a `maxconn`, HAProxy's `first` does the same; with one it moves on when the first server is full, and emfor has no connection limit. |
| `balance source`, `balance uri`, `balance hdr` | `balance = "sticky"` | The application passes the key: `lookup(name, key)`. The same key gets the same backend while it is healthy ([chapter 5](05-backend-selection.md)). |
| `balance leastconn` | none | emfor does not see the connections; see below. |
| `check port 8081` | none | emfor checks the backend's own address and port. |
| `agent-check` | a plugin or an `exec` check | There is no agent protocol; write the check as a [plugin or an `exec` check](04-health-checks.md#exec). |
| `maxconn`, queues, `observe layer7` | none | See below. |

The application then connects to what emfor tells it instead of to HAProxy.
Pick the way that suits it:

```console
$ emfor lookup api.svc.emfor
10.0.0.11:8080
$ curl "http://$(emfor lookup api.svc.emfor)/healthz"
```

or the client library of its language
([chapter 6](06-clients.md)), or the DNS name `api.svc.emfor`: for that add
a `[dns]` table to the config (the example above has none, and the DNS
server runs only when the table is present) and forward the zone from the
host resolver ([chapter 7](07-dns.md)). Limits to know about: an A or AAAA
answer carries no port, so the application keeps the port itself (or asks
for SRV); a runtime or resolver that caches answers delays a change by its
cache time ([caching](07-dns.md#caching)); and `emfor lookup` is a command
that prints one backend per process, so a program that connects to
`$(emfor lookup ...)` needs a small wrapper that runs it for each connection
([chapter 6](06-clients.md#from-the-shell-emfor-lookup)).

### Trying it next to HAProxy

emfor does not touch the traffic, so you can run it beside HAProxy first:

1. Write the services into `emfor.toml` (with the deb or rpm, into a file of
   `/etc/emfor/conf.d/`, which the packaged main config reads) and check the
   config with `emfor check-config`.
2. Start the daemon (the deb has started it at install; after a change to
   a file, `systemctl reload emfor`) and compare `emfor status` with the HAProxy stats page:
   the same backends should be healthy and unhealthy, and the reasons in
   `emfor status` say why a check failed.
3. Look at what an application would get, and why, with
   `emfor lookup api.svc.emfor --explain` ([chapter 6](06-clients.md#from-the-shell-emfor-lookup)).
4. Move one application at a time to emfor, watching the
   [metrics](08-operations.md#metrics) and `emfor status --watch`.
5. Remove a backend from HAProxy's configuration only when no application
   uses it any more.

## From Consul

A Consul service with an HTTP check, as the agent of the node that runs it
registers it:

```hcl
service {
  name = "api"
  port = 8080
  check {
    http                     = "http://localhost:8080/healthz"
    interval                 = "10s"
    timeout                  = "1s"
    success_before_passing   = 2
    failures_before_critical = 3
  }
}
```

The same job in emfor is a service that lists the nodes, and checks them
from the host that asks:

```toml
[services."api.service.consul"]
check = "http"
interval = "10s"
timeout = "1s"
rise = 2
fall = 3
backends = ["10.0.0.11:8080", "10.0.0.12:8080", "10.0.1.11:8080"]

[services."api.service.consul".check_options]
path = "/healthz"
expect_status = ["200-299"]

[dns]
zone = "service.consul"
```

The service is named `api.service.consul` and the zone is `service.consul`,
so a program that looks up `api.service.consul` keeps working once the host
resolver forwards that zone to emfor instead of to Consul ([chapter 7](07-dns.md)).
(You can also keep emfor's own zone, `svc.emfor`, and change the names.)

| Consul | emfor | Notes |
| --- | --- | --- |
| `service { name, port }` | a service and its `backends` | There is no registration: the list of backends is in the config of each host, written by whatever installs the config (Ansible, Puppet, a template). Adding a node means changing the files and reloading ([what a reload changes](03-configuration.md#what-a-reload-changes)). |
| `check { http = ... }` | `check = "http"` and `check_options` | The URL is split: the address is the backend's, the path is `path`. Consul checks usually run on the node itself (`localhost`); emfor connects to the backend's address from the application host, so a firewall must allow it. |
| `check { tcp = ... }` | `check = "tcp"` | |
| `check { grpc = ... }` | the `grpc` plugin of `emfor-check` | See [plugins](04-health-checks.md#plugins-and-emfor-check). |
| `check { args = [...] }` (script) | `check = "exec"` | |
| `interval` | `interval` | Consul has no default; emfor's is 2 seconds. |
| `timeout` | `timeout` | Consul's default for HTTP and TCP is 10 seconds; emfor's is 1 second and cannot exceed `interval`. |
| `success_before_passing` | `rise` | Consul's default is 0, where the first result decides. emfor refuses `rise = 0` (`rise must be at least 1`): `rise = 1` is the same, and emfor's default is 2. |
| `failures_before_critical` | `fall` | Consul's default is 0, where the first result decides. emfor refuses `fall = 0` (`fall must be at least 1`): `fall = 1` is the same, and emfor's default is 3. |
| `method`, `header` | none | emfor sends `GET`; of the headers only `Host` can be set (`host_header`). |
| `tls_skip_verify = true` | `tls_verify = false` | Together with `scheme = "https"`. |
| Status `passing`, `warning`, `critical` | `healthy`, `unhealthy`, `unknown` | A Consul HTTP check is `passing` for 200 to 299, `warning` for 429 and `critical` otherwise; emfor has no `warning`: set `expect_status` to what you accept. |
| `status` (initial `critical`) | starts `unknown` | A backend becomes `healthy` after `rise` passes. Until then an `open` service still returns an address ([chapter 5](05-backend-selection.md#when-no-backend-is-healthy)). |
| `consul maint -enable`, `enable_maintenance` | `drain = true` on the backend | Then reload; see [draining a backend](05-backend-selection.md#draining-a-backend). |
| `deregister_critical_service_after` | none | The list is static; nothing deregisters a backend. |
| TTL and alias checks | none | emfor only runs its own checks. |
| `api.service.consul` | the service name, with `[dns] zone` | See above. |
| `_api._tcp.service.consul` (SRV) | `_<anything>._tcp.<service name>` | emfor's SRV owner names contain the whole service name, for example `_http._tcp.api.service.consul` ([chapter 7](07-dns.md)). |
| tags, datacenters | `site` and `balance = "locality"` | A backend gets a `site`, and a host prefers the backends of its own site ([chapter 5](05-backend-selection.md)). |

Consul reads a directory of service files, one per service. emfor can do
the same with `include_dir`: put `include_dir = "conf.d"` as the first line of
`emfor.toml`, before any table (after `[dns]` it is an error,
`dns.include_dir: unknown key`), create the directory, and put one file per
service in it, for example `/etc/emfor/conf.d/10-web.toml`:

```toml
[services."web.svc.emfor"]
check = "http"
backends = ["10.0.0.21:8080", "10.0.0.22:8080"]

[services."web.svc.emfor".check_options]
path = "/healthz"
```

The directory and its files must not be writable by group or others and
must be owned by root or the user that runs emfor
([permissions](03-configuration.md#splitting-the-config-over-a-directory)).
`emfor check-config` lists the files it read, and a reload re-reads them
all ([chapter 3](03-configuration.md#splitting-the-config-over-a-directory)).
Unlike Consul, emfor does not register anything: the file is the list of
backends.

### Trying it next to Consul

The steps are those of the HAProxy section: write the config, run
`emfor check-config`, start the daemon beside the Consul agent, compare the
health `emfor status` shows with what `consul catalog` and the Consul UI
show, then move one application at a time. A difference between the two
is the useful part: it means the backend looks different from where the
application is than from where the Consul agent is.

## What does not carry over

| You use | In emfor |
| --- | --- |
| Least connections, connection limits, queues | No. emfor is not in the data path and does not see connections. Keep HAProxy for this. |
| Retries and timeouts of the proxy | The application retries. The client returns one address per lookup, so a retry gets the same backend until its next check fails, which takes about one `interval`, if the service is `sticky` or `failover`; with `round-robin` a retry in the same process takes the next candidate. A backend is left out of the candidates at its first failed check, not only once `fall` checks have marked it down ([chapter 5](05-backend-selection.md#a-backend-whose-checks-start-failing)). |
| Passive health checks (`observe layer7`) | No: only active checks. A failing backend is noticed by the next checks; with a [peer group](03-configuration.md#peers), when one member sees a backend go from `healthy` to `unhealthy` (after `fall` failed checks) it tells the others, which recheck it at once and every 200ms, up to `fall` checks. |
| Taking a backend out for maintenance (`disabled`, `maint`, `drain`) | `drain = true` on the backend, then reload ([chapter 5](05-backend-selection.md#draining-a-backend)): it is never chosen and its checks go on. `weight = 0` is refused (`weight must be from 1 to 100`), and there is no run-time command (`set server ... state drain`): the config is the only way. |
| Routing by URL, header or cookie, TLS termination | No. |
| Registration, tags and key-value store of Consul | No. The list of backends is config. |
| Several configuration files | One file for now. |
| A central view of all checks | Each host has its own. Use the [metrics](08-operations.md#metrics) with Prometheus. |

Where a row says "keep HAProxy", it is also fine to use both: emfor chooses
among several HAProxy instances, and each HAProxy chooses among the
servers.
