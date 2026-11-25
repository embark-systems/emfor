# 2. Quick start

[Contents](README.md) · Previous: [1. What emfor is](01-what-emfor-is.md) · Next: [3. Concepts](03-concepts.md)

In ten minutes, on one Linux host, you will:

1. start two small web servers to act as backends;
2. tell emfor to check them;
3. ask emfor which one to use, from the shell, over DNS and from Python;
4. stop one backend and watch emfor take it out.

You need root on a Linux machine (a virtual machine or a container is fine)
with `python3` and `curl`. `dig` is optional. The commands use the addresses
`127.0.0.1` and `127.0.0.2`, which every Linux system has on its loopback
interface. (On FreeBSD, `127.0.0.2` needs `ifconfig lo0 alias 127.0.0.2/32`
first; this walk-through was run on Linux only.)

## Step 1: install emfor

Pick one:

- **From a deb or rpm package.** The project does not publish packages; build
  them from a checkout with `packages/build.sh` (it needs Docker, see
  [chapter 4](04-installing.md#packages-deb-and-rpm)), then install the one for
  your system. The deb starts the daemon right away, with no services yet.
- **From source.** With a Rust toolchain (1.85 or later):

  ```sh
  $ cargo build --release --bin emfor --bin emfor-check
  # install -m 0755 target/release/emfor target/release/emfor-check /usr/bin/
  ```

Check that it runs:

```console
$ emfor --version
emfor 0.8.0
```

## Step 2: start two backends

Two web servers on the same port, one on each address, in a second terminal:

```sh
$ mkdir -p /tmp/web1 /tmp/web2
$ echo "hello from web1" > /tmp/web1/index.html
$ echo "hello from web2" > /tmp/web2/index.html
$ python3 -m http.server 8080 --bind 127.0.0.1 --directory /tmp/web1 &
$ python3 -m http.server 8080 --bind 127.0.0.2 --directory /tmp/web2 &
```

## Step 3: describe the service

A **service** is a name and a list of **backends**. This one is called
`web.svc.emfor`, and emfor checks it with an HTTP `GET /`.

**If you built from source**, create the directories first (the daemon does
not create them), then write this to `/etc/emfor/emfor.toml`:

```sh
# install -d -m 0755 /etc/emfor /var/lib/emfor
```

```toml
# /etc/emfor/emfor.toml
[dns]        # answer DNS queries on 127.0.0.1:5300
[metrics]    # serve Prometheus metrics on 127.0.0.1:5301

[services."web.svc.emfor"]
check = "http"
backends = ["127.0.0.1:8080", "127.0.0.2:8080"]

[services."web.svc.emfor".check_options]
path = "/"
```

```sh
# chmod 0644 /etc/emfor/emfor.toml
```

The config file must not be writable by group or others, and the store
directory `/var/lib/emfor` must exist (the command above made it).

**If you installed a package**, leave `/etc/emfor/emfor.toml` as it is (it
reads every `.toml` file in `/etc/emfor/conf.d/`) and write the same text to
`/etc/emfor/conf.d/10-web.toml` instead, with mode 0644.

Now let emfor check the file:

```console
# emfor check-config
config ok: 1 services, 2 backends
```

With a package, `check-config` also prints the files it read.

## Step 4: start the daemon

**From source**, run it in the foreground in a third terminal (it logs there):

```console
# emfor server
INFO emfor_daemon::cli: DNS server for zone svc.emfor on 127.0.0.1:5300
INFO emfor_daemon::cli: metrics on 127.0.0.1:5301
INFO emfor_daemon::cli: emfor started: 1 services, 2 backends, store /var/lib/emfor/state.mdb
INFO emfor_daemon::schedule: backend 127.0.0.1:8080 of web.svc.emfor changed from unknown to healthy (check passed) ...
INFO emfor_daemon::schedule: backend 127.0.0.2:8080 of web.svc.emfor changed from unknown to healthy (check passed) ...
```

(The log lines start with a timestamp, left out here.)

**With a package**, the daemon runs already (deb) or you start it
(`systemctl enable --now emfor` for the rpm). Load the new file with
`systemctl reload emfor`, and read the log with `journalctl -u emfor`.

## Step 5: ask emfor

`emfor status` shows what the daemon has published:

```console
# emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.0s ago, up 2.0s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy  1.7s ago  8.4ms
  127.0.0.2:8080  healthy  1.6s ago  7.5ms
```

`emfor lookup` prints the address an application would get. The service uses
`round-robin`, so two runs may print different backends:

```console
# emfor lookup web.svc.emfor
127.0.0.1:8080
# curl "http://$(emfor lookup web.svc.emfor)/"
hello from web2
```

The DNS server gives the same answer to any program:

```console
$ dig +short @127.0.0.1 -p 5300 web.svc.emfor A
127.0.0.1
$ dig +short @127.0.0.1 -p 5300 web.svc.emfor SRV
0 1 8080 127-0-0-1.web.svc.emfor.
0 1 8080 127-0-0-2.web.svc.emfor.
```

An A record carries no port; the SRV records list every healthy backend with
its port.

These commands ran as root. As a normal user `emfor status` and
`emfor lookup` fail with `Permission denied`: since 0.7.0 the store is
readable only by the daemon's user and group (see
[chapter 12](12-operating.md#who-can-read-the-store)). With a package, add your
user to the `emfor` group (`usermod -aG emfor $USER`, then log in again).

## Step 6: break a backend

Stop the second web server:

```sh
$ kill %2
```

About six seconds later (three failed checks, two seconds apart), emfor
marks it unhealthy:

```console
# emfor status
store: /var/lib/emfor/state.mdb
daemon: running, heartbeat 0.0s ago, up 9.0s, dns 127.0.0.1:5300, metrics 127.0.0.1:5301

web.svc.emfor  round-robin  on_all_down=open
  127.0.0.1:8080  healthy    1.0s ago  7.2ms
  127.0.0.2:8080  unhealthy  1.0s ago  -      error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
```

Every lookup now returns the first backend. `--explain` says why:

```console
# emfor lookup web.svc.emfor --explain
service: web.svc.emfor
answering: as a client library answers
store: /var/lib/emfor/state.mdb (heartbeat 0.0s ago, fresh)
balance: round-robin  on_all_down: open
tier: healthy primary backends whose last check passed
candidates:
  127.0.0.1:8080
excluded:
  127.0.0.2:8080  unhealthy
selection: round-robin: the candidates in turn
  the actual choice depends on the rotation start of the process, which is random in a new process
answer: 127.0.0.1:8080
```

Start the server again (`python3 -m http.server 8080 --bind 127.0.0.2 --directory /tmp/web2 &`).
After two passed checks it is `healthy` again, and `emfor status --history`
shows both changes:

```console
# emfor status --history --service web.svc.emfor
...
web.svc.emfor
  127.0.0.1:8080
    2026-11-21 17:17:33 unknown -> healthy
  127.0.0.2:8080
    2026-11-21 17:17:45 unhealthy -> healthy
    2026-11-21 17:17:39 healthy -> unhealthy  error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)
    2026-11-21 17:17:33 unknown -> healthy
```

## Step 7: look up from a program

Applications use a client library, which needs `libemfor` (see
[chapter 4](04-installing.md#client-libraries)). In Python:

```python
import emfor

addr = emfor.lookup("web.svc.emfor")   # "127.0.0.1:8080"
```

Call `lookup` before every new connection, not once at start-up: the answer
follows the checks, and a lookup costs about a microsecond.
[Chapter 9](09-clients.md) has the same for every language.

## Step 8: clean up

Stop `emfor server` with Ctrl-C (or `systemctl stop emfor`), and the two web
servers with `kill %1 %2`. The store stays in `/var/lib/emfor`; it is rebuilt
at every start, so you can delete the directory.

## What to read next

- [Chapter 3](03-concepts.md) explains what you just saw: health states, the
  store, the tiers a lookup chooses from.
- [Chapter 5](05-configuring.md) and [chapter 6](06-health-checks.md) show how
  to write a real config.
- [Chapter 4](04-installing.md) installs emfor properly, under systemd.
