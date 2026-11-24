#!/bin/sh
# Send an SNMP trap for each backend state change that emfor recorded since
# the last run, read from `emfor status --json` (`history`). Run it from cron
# or a systemd timer every few seconds to a minute; emfor keeps the last 20
# changes of each backend, so a run must come before 20 more happen.
# The recipe is docs/manual/14-integrations.md, "Old network management
# systems: SNMP and hooks".
#
#   emfor-snmp-trap.sh -H HOST -o OID [-c COMMUNITY] [-S STATEFILE] [-C CONFIG] [-e EMFOR]
#
#   -H HOST       where to send the trap (host or host:port)
#   -o OID        the base OID of your own enterprise number, for example
#                 1.3.6.1.4.1.99999.1; the trap OID is OID.1 and its five
#                 values are OID.2 (service) to OID.6 (reason)
#   -c COMMUNITY  SNMPv2c community (default: public)
#   -S STATEFILE  where to remember the newest change sent (default:
#                 /var/lib/emfor-snmp-trap/last); the first run sends nothing
#                 and only records the newest change
#   -C CONFIG     the emfor config, for `emfor status --config`
#   -e EMFOR      the emfor binary (default: emfor)
#
# Limit: the newest change sent is remembered by its time (`at`, in
# milliseconds). If two backends change in the same millisecond and a run sees
# only one of them, the other is not sent. Check times are per backend, so
# this is rare.
#
# The command that sends is `snmptrap` from net-snmp; set SNMPTRAP to another
# command to test (the arguments are those of snmptrap). Needs jq.
set -u

emfor=emfor
host=
oid=
community=public
state=/var/lib/emfor-snmp-trap/last
config=
while getopts 'H:o:c:S:C:e:' opt; do
    case "$opt" in
        H) host=$OPTARG ;;
        o) oid=$OPTARG ;;
        c) community=$OPTARG ;;
        S) state=$OPTARG ;;
        C) config=$OPTARG ;;
        e) emfor=$OPTARG ;;
        *) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
    esac
done
[ -n "$host" ] && [ -n "$oid" ] || { echo "-H and -o are required" >&2; exit 2; }
trap_cmd=${SNMPTRAP:-snmptrap}

# `emfor status` exits 1 when a service is all down or the daemon is stale or
# stopped, and still prints the history: that is when a trap matters most. Only
# 2 (config or store cannot be read) or no output is a failure here.
if [ -n "$config" ]; then
    json=$("$emfor" status --json --config "$config")
else
    json=$("$emfor" status --json)
fi
rc=$?
[ "$rc" -le 1 ] && [ -n "$json" ] || exit 1

# One line per change: at, service, backend, from, to, reason (tab separated,
# oldest first).
changes=$(printf '%s' "$json" | jq -r '
    [.history | to_entries[] | .key as $s | .value.backends | to_entries[]
     | .key as $b | .value[]
     | {at, s: $s, b: $b, from, to, reason: (.reason // "-")}]
    | sort_by(.at)[] | [.at, .s, .b, .from, .to, .reason] | @tsv') || exit 1

newest=0
[ -n "$changes" ] && newest=$(printf '%s\n' "$changes" | tail -n 1 | cut -f1)

if [ ! -f "$state" ]; then
    mkdir -p "$(dirname "$state")" && printf '%s\n' "$newest" > "$state"
    exit 0
fi
last=$(cat "$state")

tab=$(printf '\t')
printf '%s\n' "$changes" | while IFS=$tab read -r at service backend from to reason; do
    [ -n "$at" ] && [ "$at" -gt "$last" ] || continue
    "$trap_cmd" -v 2c -c "$community" "$host" '' "$oid.1" \
        "$oid.2" s "$service" "$oid.3" s "$backend" "$oid.4" s "$from" \
        "$oid.5" s "$to" "$oid.6" s "$reason" || exit 1
done || exit 1
printf '%s\n' "$newest" > "$state"
