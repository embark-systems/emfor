# emfor plugin protocol, version 1

A plugin is a program that checks backends for emfor. Use one when a check needs a protocol emfor doesn't speak (Redis, MySQL, ...), or when it should keep connections open between checks. For a one-off script, the built-in `exec` checker is simpler: it runs a command per check and maps the exit status (0 up, 1/2 down, others error).

emfor starts each plugin as a long-lived child process and talks to it over stdin and stdout. A plugin can be written in any language.

emfor ships one plugin program, `emfor-check`, with redis, MySQL and PostgreSQL checks; see [docs/deploy/plugins.md](../deploy/plugins.md). To write a plugin in Rust, the `emfor-plugin` crate (crates/emfor-plugin) handles this protocol: implement its `Plugin` trait and call `emfor_plugin::run`.

## Registering a plugin

```toml
[plugins."redis"]
command = ["/usr/local/libexec/emfor-redis", "--verbose"]

[services."cache.svc.emfor"]
check = "redis"
backends = ["10.0.2.1:6379"]
[services."cache.svc.emfor".check_options]
password_file = "/etc/emfor/redis.pass"
```

- **Plugin name:** 1–63 characters of `a-z`, `0-9`, `_` and `-`. It must not be `tcp`, `http` or `exec`.
- **`command`:** an array. The first element is an absolute path to an executable file. It is run without a shell, with working directory `/`.
- **Processes:** one per declared plugin, shared by every service that uses it. emfor starts every declared plugin, used or not.
- **Config checks start plugins.** Loading a config (at startup, on `SIGHUP`, and for `emfor --check-config`) starts the plugins and asks them to validate each service's `check_options`. `--check-config` stops them again before it exits.

## Transport

- **Format:** one JSON object per line, UTF-8, ending in `\n`.
- **Direction:** emfor writes requests to the plugin's **stdin**. The plugin writes answers to its **stdout**.
- **stdout carries protocol lines only.** Flush after every line.
- **stderr is free-form.** emfor logs each line at `info` as `plugin <name>: <line>`.
- **Message type:** every object has a `type` field.
- **Shutdown:** when stdin reaches end of file, exit. emfor closes stdin to stop a plugin, and kills it if it is still running 2 seconds later.

## Messages

### hello (handshake)

emfor sends this first:

```json
{"type":"hello","protocol":1}
```

The plugin must answer within 5 seconds:

```json
{"type":"hello","protocol":1,"name":"redis","version":"0.1.0"}
```

`name` and `version` describe the plugin and are only logged. The plugin fails to start if the answer is anything else, has a different `protocol`, or does not arrive in time.

### validate

emfor sends one `validate` per service using the plugin, when a config is loaded:

```json
{"type":"validate","id":1,"options":{"password_file":"/etc/emfor/redis.pass","db":0}}
```

`options` is the service's `check_options` converted to JSON. TOML datetimes become RFC 3339 strings.

Answer with the same `id` and a list of problems, or an empty list when the options are valid:

```json
{"type":"validate","id":1,"errors":[{"key":"password_file","message":"file not found"}]}
{"type":"validate","id":2,"errors":[]}
```

- **`key`** is relative to `check_options`. emfor reports the example above as `services."cache.svc.emfor".check_options.password_file: file not found`.
- **Deadline:** answer within 5 seconds. Otherwise the config is rejected.

### check

```json
{"type":"check","id":3,"service":"cache.svc.emfor","target":{"ip":"10.0.2.1","port":6379},"options":{"password_file":"/etc/emfor/redis.pass","db":0},"timeout_ms":900}
```

Answer with the same `id`, a `result`, and an optional `reason`:

```json
{"type":"check","id":3,"result":"down","reason":"PONG not received"}
{"type":"check","id":4,"result":"up"}
{"type":"check","id":5,"result":"error","reason":"cannot read /etc/emfor/redis.pass: permission denied"}
```

| `result` | Meaning |
| --- | --- |
| `up` | The backend passed. |
| `down` | The backend failed. Counts toward `fall`. |
| `error` | The plugin could not check, for example because of a local problem. This does not count against the backend. |

Rules:
- A `down` or `error` without `reason` is recorded with reason `no reason given`.
- Any other `result` value is recorded as `error` with reason `invalid plugin response`.
- **`timeout_ms`** is the service `timeout` minus `min(100ms, timeout / 10)`. For example, 1s gives 900 and 200ms gives 180.
  - Finish within `timeout_ms`, and answer `down` (for example `"reason":"timeout"`) if the backend is too slow.
  - If no answer arrives within the service `timeout`, emfor records `error` with reason `plugin did not answer within timeout`. A slow plugin is never counted as a backend failure.

## Rules

- **`id`** is an unsigned 64-bit integer, unique per plugin process. Copy it into the answer.
- **Concurrency:** answers may be sent in any order. emfor sends checks for all services and backends that use the plugin to the same process, so handle requests concurrently. A plugin that works through them one at a time delays every other check.
- **Ignored lines:** emfor logs a stdout line that is not JSON, or has an unknown `type`, at `warn` and ignores it. It ignores an answer whose `id` it is not waiting for, for example one that arrived after the timeout.
- **Crashes:** if the plugin exits or closes stdout, every check waiting for an answer becomes `error` with reason `plugin exited`.
  - emfor restarts the plugin after 1s, then 2s, 4s, and so on, up to 30s between attempts. The delay goes back to 1s once the plugin has run for 30 seconds.
  - Until the plugin is running again, its checks are `error` with reason `plugin <name> unavailable`.
  - After a restart, options are not validated again.
- **Reload:** on `SIGHUP`, a plugin whose `command` did not change keeps running. A plugin with a new `command` is started again, and the services that use it start over from `unknown`.

## Example messages

[protocol-v1-examples.jsonl](protocol-v1-examples.jsonl) has one line per message kind. The daemon's tests parse every line, so the file always matches what emfor sends and accepts.
