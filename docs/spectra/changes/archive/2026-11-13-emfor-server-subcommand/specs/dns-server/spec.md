## MODIFIED Requirements

### Requirement: Services with backends on different ports are reported

On startup and after every successful reload, when `[dns]` is present, the daemon SHALL log one `warn` line for each service whose name is under the zone and whose backends do not all have the same port. The line SHALL contain the service name, the service's distinct backend ports in ascending order, and the words `SRV` and `port`, telling the operator that A and AAAA answers carry no port. Backends of both address families SHALL be counted together. A service whose name is not under the zone SHALL NOT get this warning. The warning SHALL NOT change DNS answers, the validity of the config, or the output and exit status of `emfor check-config`.

#### Scenario: Mixed ports in the zone

- **WHEN** `[dns] zone = "svc.emfor"` and service `api.svc.emfor` has backends `10.0.0.11:8080` and `10.0.0.12:8081`
- **THEN** after startup the log contains one `warn` line with `api.svc.emfor`, `8080, 8081`, `SRV` and `port`, and A queries for `api.svc.emfor` are answered as before

##### Example: which services are reported

| `[dns]` | Service name | Backends | Warning |
| --- | --- | --- | --- |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.11:8080`, `10.0.0.12:8081` | yes, ports `8080, 8081` |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.12:9000`, `10.0.0.11:8080`, `10.0.0.13:8080` | yes, ports `8080, 9000` |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.11:8080`, `[2001:db8::11]:8080` | no |
| zone `svc.emfor` | `api.svc.emfor` | `10.0.0.11:8080`, `[2001:db8::11]:8443` | yes, ports `8080, 8443` |
| zone `svc.emfor` | `db.internal` | `10.0.0.21:5432`, `10.0.0.22:5433` | no (outside-zone warning only) |
| absent | `api.svc.emfor` | `10.0.0.11:8080`, `10.0.0.12:8081` | no |

#### Scenario: Reload reports the new config

- **WHEN** the daemon runs with service `api.svc.emfor` whose backends all use one port (for example 8080), and a reload adds a backend on another port (for example `10.0.0.13:9090`)
- **THEN** there is no port warning before the reload, and after it the log contains one `warn` line with `api.svc.emfor` and both ports in ascending order (`8080, 9090` in the example)
