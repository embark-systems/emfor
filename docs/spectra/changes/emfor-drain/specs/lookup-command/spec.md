## ADDED Requirements

### Requirement: Explain shows drained backends

`emfor lookup --explain` SHALL list a backend with `drain` true as excluded with the reason `drained`, in the text and in the `--json` output, whatever tier the other backends come from, and SHALL NOT offer it as a candidate. When every backend is drained, the explanation SHALL show every backend as `drained`, no tier, and the lookup SHALL fail with exit status 1.

#### Scenario: Excluded with a reason

- **WHEN** backend A is drained and B is healthy
- **THEN** `--explain` lists A as `drained` and B as a candidate

#### Scenario: Everything drained

- **WHEN** every backend is drained
- **THEN** every backend is listed as `drained`, no candidate is shown, and the exit status is 1
