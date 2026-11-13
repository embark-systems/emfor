# Security policy

## Reporting a vulnerability

Please report security problems privately, through GitHub's private
vulnerability reporting: on the repository page, open **Security**, then
**Report a vulnerability**. Do not open a public issue, pull request or
discussion for a suspected vulnerability.

Include what you can of:

- the emfor version (`emfor --version`, or the release tag) and the
  operating system;
- the component: the daemon, `emfor-check`, `libemfor` or a client
  wrapper, the DNS server, `/metrics`, or the plugin protocol;
- the steps or the input that show the problem, and what an attacker
  needs (network access to a port, a local account, control of a backend
  or of a plugin);
- the config, with passwords, host names and addresses removed or
  replaced.

emfor is maintained by a small team. We aim to acknowledge a report within
7 days and to tell you within 30 days whether we accept it and when a fix
is planned. We will credit you in the advisory and the changelog unless you
ask us not to.

## Supported versions

Until 1.0, security fixes go into the latest release only, as a new patch
release (for example 0.2.1 after 0.2.0). Upgrade every component together:
`libemfor` and the clients must come from the same release as the daemon
(see [docs/compatibility.md](docs/compatibility.md)).

| Version | Security fixes |
| --- | --- |
| 0.2.x | Yes |
| 0.1.x and earlier | No |

## What emfor trusts

Knowing what emfor is designed to trust helps to decide whether something
is a vulnerability.

- **The config file and the plugin programs** are trusted: they run with
  the daemon's rights, and `exec` checks run the commands the config names.
  Protect `emfor.toml` and the programs it names like any other root- or
  service-owned configuration.
- **Backends are not trusted.** Whatever a backend or a plugin sends is
  bounded and escaped before it reaches the store, the logs, `emfor status`
  and the metrics (reasons are cut at 512 characters, control characters
  are escaped, plugin lines are limited to 65536 bytes). A backend that can
  crash the daemon, make it use memory without bound, or inject terminal
  escape sequences is a vulnerability.
- **Local users and the store.** Applications read the store through its
  lock file, so every application user needs write access to
  `<store>-lock`. Anyone who can write that file can stop the daemon's
  commits or disturb lookups. With the default `lock_file_mode = "0666"`
  that is every local user; this is documented, and hosts with untrusted
  local users should restrict the store to a group (see "Who can use the
  store" in [docs/deploy/daemon.md](docs/deploy/daemon.md)). Reports that
  only restate this are not vulnerabilities; ways around the group
  restriction are.
- **The DNS server and `/metrics`** listen on loopback by default and have
  no authentication. Exposing them to other hosts is the operator's choice;
  firewall them to the clients that need them. Remote crashes, memory
  growth or amplification through either are vulnerabilities.
- **Clients** must never crash, panic or abort the application on any
  store content; a malformed store must end in one of the four error codes.

## Disclosure

We fix the problem, publish a release, and then publish a GitHub security
advisory that names the affected versions and the fix. If you plan to
publish your own write-up, please wait until the release is out; tell us
your date and we will work towards it.
