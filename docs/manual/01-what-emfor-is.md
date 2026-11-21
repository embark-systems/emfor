# 1. What emfor is, and what it is not

[Contents](README.md) · Next: [2. Quick start](02-quick-start.md)

## The problem

Your application talks to other services: a database with a primary and a
replica, a pool of API servers, a cache, an LDAP or RADIUS server. Each of
them has several addresses, and from time to time one of them is down. The
application needs to know, before it connects, which address works.

The usual answers are a load balancer in the middle (one more hop, one more
thing that can fail) or a service registry with a central cluster (one more
system to run). emfor does it differently.

## The answer emfor gives

emfor is a small daemon that runs **on every application host**. Each daemon:

1. reads a list of services and their backends (IP address and port) from its
   own config file;
2. checks every backend, every two seconds by default, with a check you choose
   (a TCP connect, an HTTP request, a database login, a script);
3. writes the result into a **state store**: a local file (an LMDB database)
   on the same host.

When the application needs a backend, it calls the emfor **client library**
for its language: `lookup("db.svc.emfor")`. The library reads the local store
and returns one healthy address, such as `10.0.3.1:5432`. Nothing goes over
the network, and a lookup takes about a microsecond, so the application can
ask before every new connection.

```
                    checks (tcp, http, plugins, scripts)
     backends  <------------------------------------------  emfor daemon
   10.0.3.1:5432                                                |  writes
   10.0.3.2:5432                                                v
                                                   /var/lib/emfor/state.mdb
                                                                |  reads (about 1 µs)
                                                                v
                                     application + client library  ---->  10.0.3.1:5432
```

Programs that cannot use a client library can ask emfor's optional DNS server
instead (A, AAAA and SRV records), or run `emfor lookup <service>` from a
shell script.

## What else it does

- **Choosing among healthy backends**: round-robin, sticky (by a key you
  pass), failover, weighted, by data centre (locality), or fastest.
- **Backups, drain and slow start**: hold a backend back until the others
  fail, take one out for maintenance while its checks go on, or ramp up one
  that has just recovered.
- **Peer groups**: the daemons of one data centre can share check results, so
  a failure seen by one is rechecked by all at once, and a majority vote
  decides.
- **Plugins**: `emfor-check` checks redis, MySQL/MariaDB, PostgreSQL, LDAP,
  DNS, RADIUS, SIP, Diameter, SMTP, AMQP, MongoDB, gRPC and QUIC/HTTP/3. You
  can write your own plugin, or run any script as an `exec` check.
- **Operations**: `emfor status`, `emfor lookup --explain`,
  `emfor check-config`, Prometheus metrics, packages (deb, rpm) and an example
  FreeBSD port.

## What happens when something fails

| What fails | What the application gets |
| --- | --- |
| One backend | After 3 failed checks (about 6 seconds with the defaults) it is no longer returned. The first failed check already moves lookups to the others while any other backend passes. |
| Every backend of a service | By default (`on_all_down = "open"`) still an address, because the checks may be wrong; or, with `on_all_down = "closed"`, the error `ALL_DOWN`. |
| The emfor daemon on this host | After 3 seconds the store counts as **stale**: the library no longer trusts it and behaves as if no backend were healthy (an `open` service still returns an address). A lookup never blocks. |
| The network of this host | The checks fail, and the applications on this host could not reach the backends anyway. |
| Another host, or a central server | Nothing: there is no central server, and each host decides for itself. |

## What emfor is not

emfor is **not in the data path**. It never sees a request or a connection.
That makes it fast and simple, and it also means it cannot do what a proxy
does with traffic:

- no least-connections balancing, no connection limits, no queues;
- no retries of a failed request (the application retries, and its next
  lookup gets another backend);
- no passive health checks (marking a server down because real requests
  failed): only active checks;
- no TLS termination, no routing by URL, header or cookie.

emfor is also **not a service registry**. Backends do not register
themselves. The list of backends is in each host's config file, written by
you or by your configuration management. It has no key-value store and no
central view; each host has its own (use Prometheus to see all of them).

If you need a proxy's features, keep the proxy. emfor can still decide which
backends the proxy uses: see [chapter 14](14-integrations.md).

## When emfor fits

- The backends are a known list of IP addresses: virtual machines, bare
  metal, databases, AAA servers.
- You want each application host to decide from where it stands, without
  a central component that all hosts depend on.
- The application can ask emfor before it connects: through a client library
  (Rust, C, Go, Python, Perl, PHP), through DNS, or through `emfor lookup`.

## Platforms

Linux is the main platform; the packages are deb and rpm. FreeBSD is
supported through an example port, which has been run on FreeBSD 15.1 (arm64,
in a virtual machine); FreeBSD 13.x and 14.x, and amd64 beyond the project's
CI job, have not been tested by hand. macOS is for development only.

Next: [2. Quick start](02-quick-start.md)
