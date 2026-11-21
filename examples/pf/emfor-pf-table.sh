#!/bin/sh
# Keep a pf(4) table equal to the backends emfor offers for a service, so that
# a `rdr` rule spreads connections over exactly those backends (FreeBSD).
# The recipe is docs/manual/14-integrations.md, "pf (FreeBSD)".
#
#   emfor-pf-table.sh once SERVICE TABLE [DNS_SERVER [PORT]]   one pass
#   emfor-pf-table.sh loop SERVICE TABLE [DNS_SERVER [PORT]]   a pass every second
#                                                              (SLEEP=<seconds>)
#
# DNS_SERVER and PORT are emfor's DNS server (default 127.0.0.1 and 5300). The
# script asks it for the SRV records of SERVICE, takes the targets with the
# lowest priority (a failover service puts its first backend there, a backup
# is left out while a primary is offered), reads the address out of each
# target name (10-0-0-11.api.svc.emfor is 10.0.0.11) and makes TABLE hold
# exactly those addresses with `pfctl -T replace`. It changes the table only
# when the set changed. The port of the records is not used: all backends of
# the service must have the same port, which is the one in the rdr rule.
#
# When emfor answers but has nothing to offer (SERVFAIL: every backend down
# with on_all_down = "closed", or drained) the table is emptied. pf drops a
# connection that an rdr rule cannot redirect, so a client waits for its own
# timeout; to have it refused at once, set EMPTY to an address where nothing
# listens on the rdr port (EMPTY=127.0.0.1), and the table holds that address
# instead of nothing. When emfor does not answer at all (stopped, or not
# reachable) the table is left as it is: a monitor that is away says nothing
# about the backends. Needs drill(1) from the base system, awk and pfctl, and
# the rights to change the table (root).
set -uf   # no pathname expansion: the addresses below are split, not globbed

die() { echo "emfor-pf-table: $*" >&2; exit 1; }

# Shell functions of awk, to tell an address from anything else: an IPv4
# address (four numbers up to 255) or an IPv6 one (eight groups of 1 to 4 hex
# digits, which is how emfor writes them). Nothing else goes to pfctl.
AWK_ADDR='
function v4(h,   n, i, o) {
    n = split(h, o, ".")
    if (n != 4) return 0
    for (i = 1; i <= 4; i++)
        if (o[i] !~ /^[0-9]+$/ || length(o[i]) > 3 || o[i] + 0 > 255) return 0
    return 1
}
function v6(h,   n, i, g) {
    n = split(h, g, ":")
    if (n != 8) return 0
    for (i = 1; i <= 8; i++)
        if (g[i] !~ /^[0-9A-Fa-f]+$/ || length(g[i]) > 4) return 0
    return 1
}
'

is_addr() { # <text>
    awk -v a="$1" "$AWK_ADDR"'BEGIN { exit !(v4(a) || v6(a)) }'
}

# the addresses of the lowest-priority SRV records, one a line, sorted; the
# word SERVFAIL, NXDOMAIN or NONE for an answer without addresses; and the
# word NOANSWER when there was no answer at all
offered() { # <service> <server> <port>
    out=$(drill -p "$3" SRV "_x._tcp.$1" "@$2" 2>/dev/null) || { echo NOANSWER; return; }
    case "$out" in *"rcode:"*) ;; *) echo NOANSWER; return ;; esac
    rcode=$(printf '%s\n' "$out" | sed -n 's/.*rcode: \([A-Z]*\).*/\1/p' | head -1)
    case "$rcode" in
        NOERROR) ;;
        *) echo "$rcode"; return ;;
    esac
    addrs=$(printf '%s\n' "$out" | awk "$AWK_ADDR"'
        $3 == "IN" && $4 == "SRV" {
            n = split($8, l, ".")           # first label of the target name
            host = l[1]; prio = $5
            shown = substr($8, 1, 60); gsub(/[^A-Za-z0-9._:-]/, "?", shown)
            if (host ~ /^[0-9]+-[0-9]+-[0-9]+-[0-9]+$/) { gsub(/-/, ".", host); ok = v4(host) }
            else { gsub(/-/, ":", host); ok = v6(host) }   # an IPv6 address
            if (!ok) {
                print "emfor-pf-table: ignoring SRV target " shown ": not an address" > "/dev/stderr"
                next
            }
            if (best == "" || prio + 0 < best + 0) { best = prio; list = "" }
            if (prio + 0 == best + 0) list = list host "\n"
        }
        END { printf "%s", list }' | sort -u)
    if [ -z "$addrs" ]; then echo NONE; else printf '%s\n' "$addrs"; fi
}

once() { # <service> <table> <server> <port>
    got=$(offered "$1" "$3" "$4")
    case "$got" in
        NOANSWER) echo "emfor-pf-table: emfor does not answer; $2 is left as it is" >&2; return 0 ;;
        SERVFAIL | NXDOMAIN | NONE) want=${EMPTY:-} ;;
        *) want=$got ;;
    esac
    have=$(pfctl -t "$2" -T show 2>/dev/null | sed 's/^[[:space:]]*//' | sort -u)
    [ "$want" = "$have" ] && return 0
    if [ -z "$want" ]; then
        err=$(pfctl -t "$2" -T flush 2>&1) || die "pfctl could not flush $2: $(printf '%s\n' "$err" | head -1)"
        echo "emfor-pf-table: $2 emptied ($got)" >&2
    else
        # $want holds only addresses that passed is_addr (no globbing: set -f)
        # shellcheck disable=SC2086
        err=$(pfctl -t "$2" -T replace $want 2>&1) || die "pfctl could not replace $2: $(printf '%s\n' "$err" | head -1)"
        echo "emfor-pf-table: $2 = $(echo $want)" >&2
    fi
}

mode=${1:-}
service=${2:-}
table=${3:-}
server=${4:-127.0.0.1}
port=${5:-5300}
[ -n "$service" ] && [ -n "$table" ] || { sed -n '2,29p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
case "$table" in
    -* | *[!A-Za-z0-9_-]*) die "$table is not a pf table name" ;;
esac
[ -z "${EMPTY:-}" ] || is_addr "$EMPTY" || die "EMPTY is not an IPv4 or IPv6 address"
command -v drill >/dev/null 2>&1 || die "drill is not installed (it is part of the FreeBSD base system)"
case "$mode" in
    once) once "$service" "$table" "$server" "$port" ;;
    loop) while :; do once "$service" "$table" "$server" "$port"; sleep "${SLEEP:-1}"; done ;;
    *) sed -n '2,29p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac
