## MODIFIED Requirements

### Requirement: Status watch mode

`emfor status --watch` SHALL redraw the text output every second until the operator presses `q`, `Q` or Ctrl-C, or it receives `SIGINT`, `SIGTERM` or `SIGHUP`, then restore the terminal (main screen, cursor shown, line wrapping on, raw mode off) and exit 0. The terminal SHALL also be restored when the command ends because of a panic. A backend row whose health state or reason changed since the previous frame, and a service line whose `all_down` changed, SHALL be highlighted for one frame; ages and latency changing alone SHALL NOT highlight a row. Below the services it SHALL list the most recent 10 changes, newest first, taken together from the stored history of the listed services (see "Backend state history") and from the daemon state changes seen during this run. A backend change SHALL be shown as `<HH:MM:SS> <service> <backend id> <old state> -> <new state>` using its stored `at`, and a daemon change as `<HH:MM:SS> daemon <old> -> <new>`; a change from before the current local day SHALL be prefixed with `<YYYY-MM-DD> `. The list SHALL therefore show changes that happened before `--watch` started. A failed read SHALL be shown as an error line in place of the services and retried on the next refresh instead of ending the command. When standard output is not a terminal, `emfor status --watch` SHALL print `--watch needs a terminal` and exit 2. `--watch` SHALL NOT combine with `--json`.

The `store:` line and the daemon line SHALL stay at the top of the screen; everything after them (the services, the blank line, `recent changes:` and the changes) is the scrollable content. Each line SHALL take exactly one screen row: lines wider than the terminal are cut at the right edge instead of wrapping. When the top lines and the content fit in the terminal height, the screen SHALL show them without a footer. Otherwise the last row SHALL be the footer `-- lines <first>-<last> of <total>  PgUp/PgDn Up/Down Home/End  q quit --`, where `<total>` is the number of content lines and `<first>`-`<last>` the 1-based range shown, and the content area SHALL be the rows between the top lines and the footer (at least one row). PageDown and the space bar SHALL scroll down by the height of the content area, PageUp up by the same amount, the down and up arrow keys by one line, Home to the first content line and End so that the last content line is shown. The scroll position SHALL be kept across refreshes and SHALL be limited after every refresh, key and terminal resize so that the content area never starts before the first line or shows rows past the last line while more lines are above. A terminal resize SHALL redraw at the new size without waiting for the next refresh. Other keys SHALL be ignored.

#### Scenario: Backend goes down while watching

- **WHEN** `emfor status --watch` is running and backend `10.0.0.12:8080` of `api.svc.emfor` turns from healthy to unhealthy at 14:03:12
- **THEN** the next frame highlights that backend's row and the change list starts with `14:03:12 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy`

#### Scenario: Earlier changes shown at start

- **WHEN** backend `10.0.0.12:8080` of `api.svc.emfor` turned unhealthy at 09:15:00 today and an operator starts `emfor status --watch` at 10:00:00
- **THEN** the first frame's change list contains `09:15:00 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy`

##### Example: merged change list

- **GIVEN** stored entries for `api.svc.emfor` backend `10.0.0.12:8080` at 2026-11-12 23:50:00 (`healthy -> unhealthy`) and 2026-11-13 09:15:00 (`unhealthy -> healthy`), and the daemon went from `running` to `stale` at 2026-11-13 10:00:05 while watching on 2026-11-13
- **WHEN** the frame is drawn
- **THEN** the change list is, in order, `10:00:05 daemon running -> stale`, `09:15:00 api.svc.emfor 10.0.0.12:8080 unhealthy -> healthy`, `2026-11-12 23:50:00 api.svc.emfor 10.0.0.12:8080 healthy -> unhealthy`

#### Scenario: Watch without a terminal

- **WHEN** an operator runs `emfor status --watch > out.txt`
- **THEN** the command prints `--watch needs a terminal` and exits 2

#### Scenario: Output fits the terminal

- **WHEN** the terminal has 24 rows and the top lines and content take 15 lines
- **THEN** all 15 lines are shown from the top and no footer is shown

#### Scenario: Scrolling a long list

- **WHEN** the terminal has 24 rows, the two top lines are shown, and the content has 60 lines
- **THEN** the content area is 21 rows, the first frame shows content lines 1-21 with the footer `-- lines 1-21 of 60  PgUp/PgDn Up/Down Home/End  q quit --`, and the `store:` and daemon lines stay on the first two rows after every key

##### Example: keys from the first frame

| Keys pressed | Footer range |
| --- | --- |
| none | `1-21` |
| PageDown | `22-42` |
| PageDown, PageDown | `40-60` |
| PageDown, PageUp | `1-21` |
| Down | `2-22` |
| Up | `1-21` |
| End | `40-60` |
| End, Home | `1-21` |
| End, Down | `40-60` |

#### Scenario: Position kept across refreshes and limited when the list shrinks

- **WHEN** the terminal has 24 rows, the content has 60 lines and the operator pressed PageDown once, and on the next refresh services were removed so the content has 30 lines
- **THEN** before the removal each refresh keeps the footer range at `22-42`, and after it the footer range is `10-30`

#### Scenario: Ending with a key

- **WHEN** `emfor status --watch` is running and the operator presses `q`, or Ctrl-C
- **THEN** the command returns to the main screen with the cursor shown and the terminal echoing typed characters again, and exits 0

#### Scenario: Ending with a signal

- **WHEN** `emfor status --watch` is running and receives `SIGTERM`
- **THEN** the command restores the terminal the same way and exits 0
