## ADDED Requirements

### Requirement: Showing the merged config

`emfor check-config --show` SHALL load and validate the config exactly as `emfor check-config` does, with the same errors and the same exit status. When the config is invalid it SHALL write nothing to standard output. When it is valid it SHALL write the merged config as TOML to standard output and nothing else, and SHALL write the usual `config ok: <n> services, <m> backends` line, and the `read <path>` lines when `include_dir` is set, to standard error. The TOML with its comments removed SHALL parse to the merged config without `include_dir`, which SHALL appear only as a comment, because its files are already merged, so that the output saved as a file checks without a `conf.d` next to it. Built-in defaults SHALL NOT be added, and no value SHALL be redacted. When `include_dir` is set, every key's value SHALL be followed, on its last line, by a comment `# <file>` naming the file that last set that key (a list or an inline table is set as a whole, so its comment names the file that set it; a multi-line string is followed by the comment after its closing quotes); when it is not set, the output SHALL begin with one comment line `# merged from <file>` and have no other comments. Without `--show`, the output of `emfor check-config` SHALL be unchanged.

#### Scenario: Valid config with fragments

- **WHEN** the main file sets `[services."api.svc.emfor"]` with `interval = "2s"` and `99-local.toml` sets `interval = "5s"` for the same service
- **THEN** standard output has the line `interval = "5s" # <path of 99-local.toml>` and the other keys of the service with the main file as their source

#### Scenario: Round trip

- **WHEN** the output of `--show` is saved as a file in another directory, without any `conf.d`, and given to `emfor check-config --config`
- **THEN** it is valid and `--show` of that file gives the same TOML without the comments

#### Scenario: Without include_dir

- **WHEN** `include_dir` is not set
- **THEN** the first line is `# merged from <main file>` and no other line has a comment

#### Scenario: Invalid config

- **WHEN** the config has an error
- **THEN** standard output is empty, standard error has the same errors as `emfor check-config`, and the exit status is 2

#### Scenario: Defaults are not shown

- **WHEN** a service sets no `interval`
- **THEN** the output has no `interval` line for it

#### Scenario: Without the flag

- **WHEN** `emfor check-config` runs without `--show`
- **THEN** its standard output and exit status are as before
