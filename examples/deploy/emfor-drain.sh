#!/bin/sh
# Take a backend out of rotation for a deploy, and put it back.
# The recipe is docs/manual/12-operating.md, "A rolling deploy with drain".
#
#   emfor-drain.sh on  FILE ADDR       set drain = true on ADDR's line in FILE,
#                                      check the config, reload emfor, and wait
#                                      until emfor shows the backend as drained
#   emfor-drain.sh off FILE ADDR       the same, removing drain
#   emfor-drain.sh wait-idle PORT [S [MS]]
#                                      on the backend host: wait until no
#                                      connection to the port has sent or
#                                      received anything for MS milliseconds
#                                      (default 3000), for at most S seconds
#                                      (default 60)
#   emfor-drain.sh wait-ready ADDR [S [SETTLE]]
#                                      wait until the checks of ADDR (also while
#                                      it is drained) have said healthy, with a
#                                      check newer than this command, for SETTLE
#                                      seconds in a row (default 10), for at
#                                      most S seconds (default 60). SETTLE
#                                      should be more than fall x interval of
#                                      the service: a backend that has just
#                                      stopped still shows healthy until its
#                                      failed checks have counted up to fall.
#
# FILE is a conf.d file in which each backend of the service is one line,
#   { addr = "10.0.0.11:8080" },
# written exactly so, with a space before the }. `on` changes it to
#   { addr = "10.0.0.11:8080", drain = true },
# and refuses (changing nothing) a line it cannot change.
#
# Run as root on the host that runs emfor; check-config runs as the daemon's
# user, so that a plugin the config declares does not run as root.
# Environment:
#   EMFOR_USER     the user emfor runs as (default "emfor")
#   EMFOR_RELOAD   how to reload (default "systemctl reload emfor"; FreeBSD:
#                  "service emfor reload")
#   EMFOR_CONFIG   the config file, passed as --config to emfor (default: the
#                  path built into emfor)
#   EMFOR_SERVICE  the service name, when ADDR is a backend of more than one
#                  service (default: the first service, in name order, that
#                  has ADDR)
# Needs jq (for status --json) and, for wait-idle, ss (iproute2). The script
# changes nothing but FILE. It writes the new line to FILE.new, moves it over
# FILE (same mode and owner), runs check-config, and puts the old FILE back if
# check-config refuses; only then does it reload.
set -eu

die() { echo "emfor-drain: $*" >&2; exit 1; }
reload=${EMFOR_RELOAD:-systemctl reload emfor}
service=${EMFOR_SERVICE:-}
emfor_cmd() { if [ -n "${EMFOR_CONFIG:-}" ]; then c=$1; shift; emfor "$c" --config "$EMFOR_CONFIG" "$@"; else emfor "$@"; fi; }

# check-config starts the plugins the config declares, so it runs as the
# daemon's user (as the service does, and as the FreeBSD rc script does), not
# as root: a plugin would otherwise run as root, outside the service's sandbox.
daemon_user=${EMFOR_USER:-emfor}
check_config() {
    set -- check-config
    if [ -n "${EMFOR_CONFIG:-}" ]; then set -- check-config --config "$EMFOR_CONFIG"; fi
    if [ "$(id -u)" -ne 0 ] || [ "$daemon_user" = root ]; then
        emfor "$@"
    elif command -v runuser >/dev/null 2>&1; then
        runuser -u "$daemon_user" -- emfor "$@"
    else
        line=
        for a in emfor "$@"; do
            line="$line '$(printf '%s' "$a" | sed "s/'/'\\\\''/g")'"
        done
        su -m "$daemon_user" -c "$line"
    fi
}

# addr with the characters that mean something in a sed pattern escaped.
pattern() { printf '%s' "$1" | sed 's/[][\.*^$/]/\\&/g'; }

# backend_field ADDR JQ-EXPRESSION: a value from `emfor status --json`.
backend_field() {
    emfor_cmd status --json 2>/dev/null |
        jq -r --arg a "$1" --arg s "$service" \
            "[.services[] | select(\$s == \"\" or .name == \$s) | .backends[] | select(.id == \$a)][0] | $2"
}

wait_for() { # SECONDS DESCRIPTION COMMAND...: poll once a second
    t=$1; what=$2; shift 2
    while ! "$@"; do
        t=$((t - 1)); [ "$t" -gt 0 ] || die "timed out: $what"
        sleep 1
    done
}

drained() { [ "$(backend_field "$1" '(.drain // false)')" = true ]; }
undrained() { [ "$(backend_field "$1" '(.drain // false)')" = false ]; }
# healthy ADDR SINCE-MS: the checks say healthy, the last check passed (not
# visible while drained: the store shows only the state of the checks then),
# and a check has finished after SINCE-MS.
healthy() {
    [ "$(backend_field "$1" "if (.drain // false) then (.checked_state // \"\")
        elif .reason == null then .state else \"failing\" end")" = healthy ] || return 1
    at=$(backend_field "$1" '(.checked_at // 0)')
    [ "$at" -gt "$2" ]
}

edit() { # on|off FILE ADDR
    mode=$1; file=$2; addr=$3
    [ -f "$file" ] || die "$file is not a file"
    p=$(pattern "$addr")
    n=$(grep -c "addr = \"$p\"" "$file" || true)
    [ "$n" = 1 ] || die "$file has $n lines with addr = \"$addr\", not 1"
    cp -p "$file" "$file.new"   # same mode and owner
    case "$mode" in
        on) sed "/addr = \"$p\"/{/drain = true/!s/ }/, drain = true }/;}" "$file" > "$file.new" ;;
        off) sed "/addr = \"$p\"/s/, drain = true//" "$file" > "$file.new" ;;
    esac
    case "$mode" in
        on) grep -q "addr = \"$p\".*drain = true" "$file.new" ;;
        off) ! grep -q "addr = \"$p\".*drain" "$file.new" ;;
    esac || { rm -f "$file.new"; die "cannot change the line of $addr in $file (it must read { addr = \"$addr\" }, with a space before the })"; }
    if cmp -s "$file" "$file.new"; then
        rm -f "$file.new"   # already as wanted; reload anyway
    else
        # A file that does not end in .toml is not read by emfor, so the
        # temporary files are never loaded.
        cp -p "$file" "$file.old"
        mv "$file.new" "$file"
        if ! check_config >/dev/null; then
            mv "$file.old" "$file"
            die "check-config refuses the new $file; the old file is back"
        fi
        rm -f "$file.old"
    fi
    sh -c "$reload"
}

cmd=${1:-}
case "$cmd" in
    on)
        edit on "${2:?FILE}" "${3:?ADDR}"
        wait_for 15 "$3 is not shown as drained" drained "$3"
        ;;
    off)
        edit off "${2:?FILE}" "${3:?ADDR}"
        wait_for 15 "$3 is still shown as drained" undrained "$3"
        ;;
    wait-ready)
        addr=${2:?ADDR}; t=${3:-60}; settle=${4:-10}
        since=$(($(date +%s) * 1000)); ok=0
        while :; do
            if healthy "$addr" "$since"; then
                [ "$ok" -gt 0 ] || ok=$(date +%s)
                [ $(($(date +%s) - ok)) -lt "$settle" ] || exit 0
            else
                ok=0
            fi
            t=$((t - 1)); [ "$t" -gt 0 ] || die "timed out: the checks of $addr have not said healthy for $settle seconds"
            sleep 1
        done
        ;;
    wait-idle)
        port=${2:?PORT}
        quiet=${4:-3000}
        command -v ss >/dev/null 2>&1 \
            || die "wait-idle needs ss (Linux, iproute2); elsewhere wait a fixed time instead"
        # Connections that a proxy keeps open for the next request stay
        # open after a drain, so "no connection" never comes. A connection
        # is busy when it moved data in the last $quiet milliseconds (`ss
        # -i` gives lastsnd and lastrcv); a request that is silent for
        # longer than that is taken for an idle one.
        busy() {
            ss -Htni state established "( sport = :$port )" | awk -v q="$quiet" '
                /lastsnd:/ {
                    s = r = 1000000000
                    for (i = 1; i <= NF; i++) {
                        if ($i ~ /^lastsnd:/) { sub(/^lastsnd:/, "", $i); s = $i + 0 }
                        if ($i ~ /^lastrcv:/) { sub(/^lastrcv:/, "", $i); r = $i + 0 }
                    }
                    if (s < q || r < q) n++
                }
                END { print n + 0 }'
        }
        # Two quiet looks in a row: a request may arrive between them.
        t=${3:-60}
        while :; do
            if [ "$(busy)" = 0 ]; then sleep 1; [ "$(busy)" = 0 ] && exit 0; fi
            t=$((t - 1)); [ "$t" -gt 0 ] || die "timed out: connections to port $port are still in use"
            sleep 1
        done
        ;;
    *)
        sed -n '2,44p' "$0" | sed 's/^# \{0,1\}//' >&2
        exit 2
        ;;
esac
