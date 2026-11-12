## Why

An A or AAAA answer carries only an IP address, so a program that finds a service through DNS must already know the port. When the backends of a service in the DNS zone listen on different ports, that program connects to the wrong port on some of them, and nothing tells the operator. emfor already warns about services outside the zone; it should warn about this case the same way, and point to SRV records (emfor-dns-srv) or a client, which do carry the port.

## What Changes

- On startup and after every successful reload, when `[dns]` is present, the daemon logs one `warn` line for each service under the zone whose backends do not all use the same port. The line names the service, lists its distinct ports in ascending order, and says that A/AAAA answers carry no port and that SRV or a client gives the port.
- The check runs next to the existing outside-zone warning: a service outside the zone gets only the outside-zone warning, because DNS does not answer for it at all.
- The warning does not change answers, the config's validity, `--check-config` output, or the exit status.

## Non-Goals (optional)

- Rejecting such configs: different ports per backend are valid, and SRV and the clients handle them.
- Printing the warning from `--check-config`: it prints no warnings today (the outside-zone warning is also startup and reload only); adding warnings there is a separate decision.
- Warning when `[dns]` is absent: nothing is answered over DNS then.
- A separate rule per address family: all backends of the service are counted together, as SRV lists them together.
- A design document: the change is one pure function and one call site next to an existing warning.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `dns-server`: adds the requirement "Services with backends on different ports are reported".

## Impact

- Affected specs: dns-server.
- Affected code: crates/emfor-daemon/src/cli.rs (the startup and reload paths that already call the outside-zone warning; a pure function returning the warning lines, unit-tested in that file).
- docs/deploy/dns-resolver.md: one sentence in "Check emfor first" about the new warning.
- docs/spectra/config.yaml: the follow-up item moves to the implemented DNS description.
- No change to the store, the config format, DNS answers, or the clients.
