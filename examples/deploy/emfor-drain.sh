#!/bin/sh
# Take a backend out of rotation for a deploy, and put it back.
# The recipe is docs/manual/08-operations.md, "Rolling deploy with drain".
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
#   emfor-drain.sh wait-ready ADDR [S] wait until the checks of ADDR say healthy
#                                      (also while it is drained)
#
# FILE is a conf.d file in which each backend of the service is one line,
#   { addr = "10.0.0.11:8080" },
# which `on` changes to  { addr = "10.0.0.11:8080", drain = true },
# Reload with EMFOR_RELOAD (default "systemctl reload emfor"). Needs jq (for
# status --json) and, for wait-idle, ss (iproute2). The script changes nothing
# but FILE, and it replaces FILE only after the line has changed, with the
# mode and owner it had: emfor refuses a conf.d file that others can write.
set -eu

die() { echo "emfor-drain: $*" >&2; exit 1; }
reload=${EMFOR_RELOAD:-systemctl reload emfor}

# addr with the characters that mean something in a sed pattern escaped.
pattern() { printf '%s' "$1" | sed 's/[][\.*^$/]/\\&/g'; }

# backend_field ADDR JQ-EXPRESSION: a value from `emfor status --json`.
backend_field() {
    emfor status --json 2>/dev/null |
        jq -r --arg a "$1" "[.services[].backends[] | select(.id == \$a)][0] | $2"
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
healthy() {
    [ "$(backend_field "$1" '(.checked_state // .state)')" = healthy ]
}

edit() { # on|off FILE ADDR
    mode=$1; file=$2; addr=$3
    [ -f "$file" ] || die "$file is not a file"
    p=$(pattern "$addr")
    n=$(grep -c "addr = \"$p\"" "$file" || true)
    [ "$n" = 1 ] || die "$file has $n lines with addr = \"$addr\", not 1"
    cp -p "$file" "$file.new"
    case "$mode" in
        on) sed "/addr = \"$p\"/{/drain = true/!s/ }/, drain = true }/;}" "$file" > "$file.new" ;;
        off) sed "/addr = \"$p\"/s/, drain = true//" "$file" > "$file.new" ;;
    esac
    # A fragment that does not end in .toml is not read by emfor, so the
    # temporary file cannot be loaded half-written.
    mv "$file.new" "$file"
    emfor check-config >/dev/null || die "check-config refuses the new $file"
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
        wait_for "${3:-60}" "$2 is not healthy" healthy "${2:?ADDR}"
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
        sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//' >&2
        exit 2
        ;;
esac
