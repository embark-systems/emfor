## ADDED Requirements

### Requirement: Config directory with ordered fragments

The main config file MAY set a top-level `include_dir` string. A relative value SHALL be resolved against the directory of the main config file; an absolute value SHALL be used as is. The daemon SHALL read the main config file first and then every selected file of that one directory, and merge them into a single config before validation, so that the main file is the lowest layer and a later file overrides an earlier one. A file SHALL be selected when its name ends in `.toml`, does not start with `.`, and it is a regular file or a symbolic link to a regular file. Selected files SHALL be read in byte-wise filename order, not locale order. A name that is not valid UTF-8 SHALL be skipped and reported as a warning. Every other entry, including subdirectories and names such as `foo.toml~`, `.x.toml.swp` and `x.toml.disabled`, SHALL be ignored. An empty directory SHALL be valid. A selected file whose type or content cannot be read, for any reason other than that it does not exist (a dangling link), SHALL be a config error that names it. An `include_dir` value that is the empty string SHALL be a config error. When `include_dir` is not set, loading and the output of `emfor check-config` SHALL be unchanged.

#### Scenario: Files are read in filename order

- **WHEN** `include_dir = "conf.d"` and the directory holds `10-b.toml`, `01-a.toml` and `9-c.toml`
- **THEN** the files are read in the order `01-a.toml`, `10-b.toml`, `9-c.toml`, after the main file

#### Scenario: Unselected entries are ignored

- **WHEN** the directory holds `a.toml`, `.hidden.toml`, `b.toml~`, `c.toml.disabled` and a subdirectory `d.toml`
- **THEN** only `a.toml` is read

#### Scenario: Symbolic link to a regular file is followed

- **WHEN** `10-svc.toml` is a symbolic link to a regular file elsewhere
- **THEN** that file is read as `10-svc.toml`

#### Scenario: Relative include_dir

- **WHEN** the main file is `/etc/emfor/emfor.toml` and `include_dir = "conf.d"`
- **THEN** the directory read is `/etc/emfor/conf.d`

#### Scenario: Non-UTF-8 file name

- **WHEN** a file in the directory has a name that is not valid UTF-8
- **THEN** it is skipped, a warning names it, and loading continues

#### Scenario: Dangling link is skipped, unreadable link is an error

- **WHEN** the directory holds `a.toml` as a link to a path that does not exist, and `loop.toml` as a link to itself
- **THEN** `a.toml` is skipped without error, and loading fails with an error that names `loop.toml`

#### Scenario: Empty include_dir

- **WHEN** the main file sets `include_dir = ""`
- **THEN** loading fails with the error `include_dir: must not be empty`

### Requirement: Fragment merge rules

When files are merged, a table in a later file and a table at the same key in an earlier layer SHALL be merged key by key, recursively. Any other combination SHALL be replaced as a whole by the later value: a scalar, a list (including `backends` and `command`), or two values of different types. A later file SHALL NOT delete a service, plugin or backend defined in an earlier layer. A service whose merged table lacks `check` or `backends` SHALL be a validation error, as in a single file. The merged config SHALL be validated, and SHALL behave in plugin startup, reload restart checks and peer group digest exactly as the same content written in one file.

#### Scenario: Table merges key by key

- **WHEN** the main file defines `[services."api.svc.emfor"]` with `check`, `backends` and `interval = "2s"`, and `99-local.toml` defines the same service with only `interval = "500ms"`
- **THEN** the service keeps `check` and `backends` and uses `interval` 500ms

#### Scenario: List is replaced

- **WHEN** the main file sets `backends = ["10.0.0.1:80", "10.0.0.2:80"]` and a later file sets `backends = ["10.0.0.3:80"]` for the same service
- **THEN** the service has the single backend `10.0.0.3:80`

#### Scenario: Plugin declared in a fragment

- **WHEN** a fragment declares `[plugins."probe"]` with a `command` and a service uses `check = "probe"`
- **THEN** the plugin is started as if declared in the main file

#### Scenario: Fragment changes a listen address

- **WHEN** a reload finds that a fragment changed `[dns] listen`
- **THEN** the reload reports that the change needs a restart, as for the same change in the main file

#### Scenario: Peer digest does not depend on file splitting

- **WHEN** a service is defined in one file, and the same service with the same values is split across two files
- **THEN** both give the same service definition digest

### Requirement: Keys allowed only in the main file

`[store]` and `include_dir` SHALL be accepted only in the main config file. A fragment that contains either SHALL be a config error naming that file. All other tables and the top-level `site` key MAY appear in fragments and SHALL be merged. `emfor status` and `emfor lookup` SHALL read only the main config file and SHALL NOT need to read the directory.

#### Scenario: Store in a fragment

- **WHEN** `10-store.toml` contains a `[store]` table
- **THEN** loading fails with an error that names `10-store.toml`

#### Scenario: Nested include

- **WHEN** a fragment sets `include_dir`
- **THEN** loading fails with an error that names that fragment

#### Scenario: status and lookup need only the main file

- **WHEN** a fragment in the directory is unreadable by the user running `emfor status`
- **THEN** `emfor status` and `emfor lookup` still work

### Requirement: Config directory permissions

The daemon SHALL refuse to load the config when the include directory, or any file read from it (for a symbolic link, the file it points to), is writable by group or others (mode `& 0o022` not zero). The error SHALL name the path and its mode. The main config file SHALL NOT get a new permission check.

#### Scenario: Group-writable fragment

- **WHEN** `10-a.toml` has mode 0664
- **THEN** loading fails with an error naming `10-a.toml` and its mode

#### Scenario: World-writable directory

- **WHEN** the include directory has mode 0777
- **THEN** loading fails with an error naming the directory

#### Scenario: Main file mode unchanged

- **WHEN** the main file has mode 0664 and `include_dir` is not set
- **THEN** loading is not refused for that reason

### Requirement: Missing config directory

An `include_dir` that does not exist or is not a directory SHALL be a config error. At daemon start the error SHALL be logged and the process SHALL exit with the config error status without opening the store. On a reload the error SHALL reject the reload, the daemon SHALL keep running with the previous config, and the error SHALL be logged.

#### Scenario: Missing directory at start

- **WHEN** the daemon starts and `include_dir` names a directory that does not exist
- **THEN** it logs the error and exits with status 2

#### Scenario: Directory removed before a reload

- **WHEN** the directory is removed and the daemon receives `SIGHUP`
- **THEN** the reload is rejected, the previous config stays in effect, and the daemon keeps running

### Requirement: Errors name their file

A syntax error in the main file or a fragment SHALL be reported with the file name, line and column. A validation error SHALL be reported with its key path and, when known, the file that last set that key or its nearest recorded parent key. Attribution SHALL be best effort when an error arises from values set in different files. An error found after validation, namely a plugin that fails to start or a change that needs a restart on reload, SHALL name the file in the same way when its key path is known. Any error in the main file or any fragment SHALL reject the whole load, and on reload the previous config SHALL be kept.

#### Scenario: Syntax error in a fragment

- **WHEN** `20-x.toml` has a TOML syntax error on line 3, column 7
- **THEN** the error reads `20-x.toml:3:7` followed by the TOML message

#### Scenario: Invalid value in a fragment

- **WHEN** `30-y.toml` sets `interval = "fast"` for a service
- **THEN** the error shows the key path `services."api.svc.emfor".interval` and the file `30-y.toml`

#### Scenario: Plugin start failure and restart-required change name their file

- **WHEN** a plugin declared in `20-probe.toml` fails to start, or a reload finds `[dns] listen` changed by `20-dns.toml`
- **THEN** the error for `plugins.probe.command` or `dns.listen` is followed by `(in <path of that file>)`

#### Scenario: One bad fragment rejects a reload

- **WHEN** one of three fragments is invalid at reload
- **THEN** none of the three is applied and the previous config stays in effect

### Requirement: Listing the files read

When `include_dir` is set, `emfor check-config` SHALL print its usual `config ok: <n> services, <m> backends` line and then one `read <path>` line per file read, the main file first, in read order. The daemon SHALL log the same list at start and after each successful reload. Warnings about skipped file names SHALL be printed on standard error by `emfor check-config` and logged by the daemon at start and at every reload, whether or not the load succeeds. When `include_dir` is not set, the output SHALL be exactly as before.

#### Scenario: check-config with fragments

- **WHEN** `include_dir` is set and two fragments are read
- **THEN** the output is the `config ok:` line followed by three `read` lines: the main file and the two fragments in order

#### Scenario: Skipped name reported by check-config and by a failed load

- **WHEN** a fragment name is not valid UTF-8 and the rest of the config is valid, or the load fails for another reason
- **THEN** `emfor check-config` writes a warning naming the skipped file to standard error, and the daemon logs it at start and reload

#### Scenario: check-config without include_dir

- **WHEN** `include_dir` is not set
- **THEN** the output is the single `config ok:` line, as before
