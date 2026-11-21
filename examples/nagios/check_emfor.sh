#!/bin/sh
# A Nagios, Icinga or Zabbix-style check of what emfor reports: the state of
# the daemon and of the backends of its services (`emfor status --json`).
# The recipe is docs/manual/08-operations.md, "Watching emfor from Nagios".
#
#   check_emfor.sh [-s SERVICE] [-w N] [-c N] [-C CONFIG] [-e EMFOR]
#
#   -s SERVICE  check only this service (default: every service)
#   -w N        WARNING when fewer than N backends of a service are healthy
#               (default: every backend that is not drained)
#   -c N        CRITICAL when fewer than N are healthy (default 1)
#   -C CONFIG   the emfor config, for `emfor status --config` (it names the store)
#   -e EMFOR    the emfor binary (default: emfor)
#
# CRITICAL: the daemon is stale or stopped, or a service has fewer than -c
# healthy backends (all down, or all drained). WARNING: fewer than -w are
# healthy. UNKNOWN: emfor or jq is not usable, the store cannot be read, or
# the service does not exist. A drained backend is left out of the counts (it
# is out on purpose) and is named in the text. Needs jq. The perfdata is
# '<service>_healthy'=<healthy>;<warn>;<crit>;0;<backends that are not drained>.
set -u

emfor=emfor
service=
warn=
crit=1
config=
while getopts 's:w:c:C:e:' opt; do
    case "$opt" in
        s) service=$OPTARG ;;
        w) warn=$OPTARG ;;
        c) crit=$OPTARG ;;
        C) config=$OPTARG ;;
        e) emfor=$OPTARG ;;
        *) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 3 ;;
    esac
done

unknown() { printf 'EMFOR UNKNOWN - %s\n' "$*" | tr '|' '/'; exit 3; }

command -v jq >/dev/null 2>&1 || unknown "jq is not installed"
command -v "$emfor" >/dev/null 2>&1 || [ -x "$emfor" ] || unknown "$emfor is not installed"
case "$warn$crit" in *[!0-9]*) unknown "-w and -c are numbers" ;; esac

# shellcheck disable=SC2086
out=$("$emfor" status --json ${config:+--config "$config"} ${service:+--service "$service"} 2>&1)
rc=$?
# 2: the config or the store cannot be read, or --service names nothing
[ "$rc" != 2 ] || unknown "$(echo "$out" | head -1)"
printf '%s' "$out" | jq -e . >/dev/null 2>&1 || unknown "emfor status gave no JSON: $(echo "$out" | head -1)"

result=$(printf '%s' "$out" | jq -r --arg w "$warn" --arg c "$crit" --arg q "'" '
  def bad: .state != "healthy";
  # Nagios reads "|" as the start of the perfdata and a line feed as the end of
  # the text, so neither may come from a service name, a backend id or a reason.
  def text: tostring | gsub("[|]"; "/") | gsub("[\n\r]"; " ");
  def perflabel: tostring | gsub("[|]"; "/") | gsub($q; $q + $q) | gsub("[\n\r]"; " ");
  . as $s
  | ($c | tonumber) as $crit
  | [ .services[]
      | . as $svc
      | ([.backends[] | select(.drain != true)]) as $live
      | ([.backends[] | select(.drain == true) | .id | text]) as $drained
      | ([$live[] | select(.state == "healthy")] | length) as $healthy
      | ($live | length) as $n
      | (if $w == "" then $n else ($w | tonumber) end) as $warn
      | { name: .name, shown: (.name | text), label: (.name | perflabel), healthy: $healthy, n: $n, warn: $warn, crit: $crit,
          drained: $drained,
          failing: [$live[] | select(bad) | "\(.id | text) \(.reason // .state | text)"],
          level: (if $healthy < $crit then 2 elif $healthy < $warn then 1 else 0 end) } ] as $svcs
  | (if $s.daemon != "running" then 2 else ([$svcs[].level] | max // 0) end) as $level
  | ["OK", "WARNING", "CRITICAL"][$level] as $word
  | ( if $s.daemon != "running"
        then "the daemon is \($s.daemon)"
        else ($svcs | map(select(.level > 0)
                 | "\(.shown): \(.healthy) of \(.n) healthy"
                   + (if (.failing | length) > 0 then " (" + (.failing | join(", ")) + ")" else "" end)
                   + (if (.drained | length) > 0 then ", drained: " + (.drained | join(" ")) else "" end))
              | join("; ")) end ) as $why
  | ( if $why != "" then $why
      else "\($svcs | length) service(s), every backend that is not drained is healthy"
           + (([$svcs[] | select(.drained | length > 0) | "\(.shown) drained: \(.drained | join(" "))"]) | if length > 0 then " (" + join("; ") + ")" else "" end)
      end ) as $text
  | "\($level)",
    "EMFOR \($word) - \($text) | " + ($svcs | map("\($q)\(.label)_healthy\($q)=\(.healthy);\(.warn);\(.crit);0;\(.n)") | join(" "))
' 2>/dev/null) || unknown "jq could not read the status of emfor"

level=$(printf '%s\n' "$result" | sed -n 1p)
echo "$(printf '%s\n' "$result" | sed -n 2p)"
exit "$level"
