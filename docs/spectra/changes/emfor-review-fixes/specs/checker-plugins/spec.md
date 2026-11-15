## ADDED Requirements

### Requirement: Cancelled requests do not count as unanswered

A request SHALL count towards a plugin having stopped answering only when its caller gave up waiting after the time it was willing to wait (for a `check` the check's timeout, for a `validate` the validation timeout). A request that is dropped earlier, because the task that made it was cancelled by a reload or by shutdown, SHALL NOT count.

#### Scenario: Reload cancels checks in flight

- **WHEN** four checks of one plugin are waiting for their answers and the daemon cancels their tasks long before their timeout
- **THEN** the plugin is not killed or restarted, and it answers the next request

#### Scenario: Callers give up after the timeout

- **WHEN** three requests in a row get no answer and their callers give up when the timeout passes
- **THEN** the plugin is killed and restarted as before

### Requirement: Bounded validation answer

A plugin library SHALL answer `validate` with at most 12 errors, each key and message cut to its first 512 characters followed by `…`, and SHALL drop errors from the end until the JSON line is at most 48 KiB (keeping at least one), so that the answer stays under the 64 KiB line limit of the daemon even when every character takes six bytes in JSON.

#### Scenario: Many long errors

- **WHEN** a plugin finds 100 errors whose messages are 2000 characters long
- **THEN** the answer has 12 errors whose keys and messages are 512 characters followed by `…`

#### Scenario: Control characters

- **WHEN** the messages are control characters, which take six bytes each in JSON
- **THEN** the answer line is under 64 KiB and has at least one error
