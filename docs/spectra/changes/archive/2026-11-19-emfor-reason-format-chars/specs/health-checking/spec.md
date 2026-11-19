## MODIFIED Requirements

### Requirement: Reasons are bounded and printable

Every check result's reason, from every checker and plugin, SHALL be normalized before it reaches the health state, the store, the logs of state changes, or the metrics: tab, carriage return and line feed SHALL become the two characters `\t`, `\r` and `\n`; every other character in Unicode category Cc (U+0000–U+001F, U+007F, U+0080–U+009F) SHALL become `\u{` followed by its code point in lowercase hex without leading zeros and `}`; the invisible format characters that hide text or change its order SHALL become the same `\u{...}` form, namely U+00AD, U+061C, U+180E, U+200B–U+200F, U+202A–U+202E, U+2060–U+2064, U+2066–U+206F, U+FEFF, U+FFF9–U+FFFB and U+E0000–U+E007F; then a reason longer than 512 characters SHALL be cut to its first 511 characters followed by `…`. All other characters, including `\`, the other characters of Unicode category Cf and ordinary non-ASCII text, SHALL stay as they are.

#### Scenario: Escape sequence from a backend

- **WHEN** an `exec` check prints `bad\x1b[2Jstate` (with a real ESC byte) and exits 1
- **THEN** the backend's reason in the store is `bad\u{1b}[2Jstate`

#### Scenario: Text direction override from a backend

- **WHEN** a check produces the reason `up<U+202E>nwod`, with a real U+202E (right-to-left override)
- **THEN** the reason in the store, in `emfor status` and in the logs is `up\u{202e}nwod`, so that no character after it is displayed reversed

##### Example: normalization

| Reason produced by the check | Stored reason |
| --- | --- |
| `connection refused` | `connection refused` |
| `a<TAB>b` | `a\tb` |
| `x<ESC>y` | `x\u{1b}y` |
| `x<U+0085>y` | `x\u{85}y` |
| `up<U+202E>nwod` | `up\u{202e}nwod` |
| `a<U+2066>b<U+2069>` | `a\u{2066}b\u{2069}` |
| `a<U+200B>b` | `a\u{200b}b` |
| `<U+FEFF>x` | `\u{feff}x` |
| `a<U+E0041>b` | `a\u{e0041}b` |
| `é ok 日本` | `é ok 日本` |
| `<U+0600>1 ñ ℃` | `<U+0600>1 ñ ℃` |
| 600 × `a` | 511 × `a` followed by `…` |
| 512 × `a` | 512 × `a` |
