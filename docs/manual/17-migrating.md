# 17. Migrating from HAProxy or Consul

[Contents](README.md) · Previous: [16. Troubleshooting](16-troubleshooting.md) · Next: [18. Configuration reference](18-config-reference.md)

emfor does one part of what HAProxy and Consul do: it checks a fixed list of
backends from each application host and tells the application which one to
use. Statements below about HAProxy and Consul describe those products as
the repository's documents do; they were not checked against them for this
manual.

| | HAProxy | Consul | emfor |
| --- | --- | --- | --- |
| Traffic goes | Through HAProxy | Straight to the backend | Straight to the backend |
| Who picks the backend | HAProxy, per connection | The application, from DNS | The application, from the local store or DNS |
| Checks run | In HAProxy | In the agent next to the service | On every application host |
| The backend list | Config | Services register | Config on each host |
| Central servers | No | Yes | No |

emfor fits when the backends are a known list of addresses (VMs, bare metal,
databases, AAA servers) and the application can ask where to connect. It
does not replace what a proxy does with traffic: least connections, retries,
queues, TLS termination, routing by URL or header, connection limits,
passive checks. Keep HAProxy for those, and optionally feed it from emfor
([chapter 14](14-integrations.md#haproxy)).

## From HAProxy

This backend:

```text
backend api
    balance roundrobin
    option httpchk GET /healthz
    http-check expect status 200
    default-server inter 2s fall 3 rise 2
    server api1 10.0.0.11:8080 check
    server api2 10.0.0.12:8080 check weight 3
    server api3 10.0.1.11:8080 check backup slowstart 30s
```

becomes:

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
expect_status = ["200"]
```

| HAProxy | emfor | Notes |
| --- | --- | --- |
| `backend api` | `[services."api.svc.emfor"]` | Also the DNS name. |
| `server ... 10.0.0.11:8080` | an entry of `backends` | IP addresses only. |
| `check` | `check = "http"` or `"tcp"` | One check per service. |
| `inter`, `fall`, `rise` | `interval`, `fall`, `rise` | Defaults 2 s, 3, 2. |
| `option httpchk GET /healthz` | `path = "/healthz"` | emfor always sends `GET`. |
| `http-check expect status 200` | `expect_status = ["200"]` | Default `200-399`. |
| `hdr Host x` | `host_header = "x"` | Only `Host` can be set. |
| `weight 3` | `weight = 3` | 1–100; `weight = 0` is refused: use `drain = true`. Used by `weighted` and by `sticky` with a key. |
| `backup` | `backup = true` | |
| `slowstart 30s` | `slow_start = "30s"` | Per service, ten steps. |
| `balance roundrobin` with weights | `balance = "weighted"` | emfor's `round-robin` ignores weights. |
| `balance first` | `balance = "failover"` | |
| `balance source`, `uri`, `hdr` | `balance = "sticky"` | The application passes the key. |
| `balance leastconn`, `maxconn`, queues, `observe layer7` | none | emfor does not see connections. |
| `check port 8081` | none | emfor checks the backend's own address and port (or use `exec` or a plugin). |
| `agent-check` | `exec` or a plugin | |
| `disabled`, `set server ... state drain` | `drain = true`, then reload | No run-time command. |

## From Consul

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

The same in emfor, keeping the Consul DNS names so programs need no change
once the host resolver forwards `service.consul` to emfor:

```toml
[dns]
zone = "service.consul"

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
```

| Consul | emfor | Notes |
| --- | --- | --- |
| Registration (`service { name, port }`) | `backends` in the config | Nothing registers; your configuration management writes the list. Add a node by changing the file and reloading. |
| `check { http = ... }` | `check = "http"` | emfor connects to the backend's address from the application host, not `localhost` on the node: open the firewall. |
| `check { tcp = ... }` | `check = "tcp"` | |
| `check { grpc = ... }` | the `grpc` check of `emfor-check` | |
| `check { args = [...] }` | `check = "exec"` | |
| `interval`, `timeout` | `interval`, `timeout` | emfor's `timeout` defaults to 1 s and cannot exceed `interval`. |
| `success_before_passing`, `failures_before_critical` | `rise`, `fall` | At least 1 in emfor (`rise = 1` is "the first result decides"). |
| `tls_skip_verify` | `tls_verify = false` | with `scheme = "https"`. |
| `passing`, `warning`, `critical` | `healthy`, `unhealthy`, `unknown` | No `warning`: list in `expect_status` what you accept. |
| Maintenance mode | `drain = true` | |
| Tags, datacenters | `site`, `balance = "locality"` | |
| `_api._tcp.service.consul` (SRV) | `_<anything>._tcp.api.service.consul` | |
| TTL and alias checks, `deregister_critical_service_after`, KV | none | |

One file per service works with `include_dir`
([chapter 5](05-configuring.md#splitting-the-config-over-a-directory)); note
that a `backends` list is replaced as a whole by the last file that sets it.

## Trying emfor next to the old system

emfor does not touch the traffic, so run both for a while:

1. Write the services, run `emfor check-config`, start the daemon.
2. Compare `emfor status` with the HAProxy stats page or the Consul UI. A
   difference is useful: the backend looks different from the application
   host than from where the other system checks it.
3. See what an application would get with `emfor lookup <service> --explain`.
4. Move one application at a time to a client library, `emfor lookup` or
   DNS, watching the metrics.
5. Remove a backend from the old system only when nothing uses it.

## What applications must do themselves

- **Retry.** A lookup returns one address. A backend drops out at its first
  failed check (when another passes), so a retry a moment later usually gets
  another one; with `round-robin` a retry in the same process gets the next
  candidate anyway.
- **Keep the port**, if they use A/AAAA DNS answers, which carry none (or use
  SRV, or a client library).
- **Look up per connection**, not once at start.
