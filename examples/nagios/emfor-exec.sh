#!/bin/sh
# Run a Nagios-style plugin as an emfor `exec` check.
# The recipe is docs/manual/08-operations.md, "Watching emfor from Nagios".
#
#   emfor-exec.sh [--warning up|down] [--host-flag F] [--port-flag F] PLUGIN [ARGS...]
#
# emfor starts a program without a shell and gives it the backend in the
# environment (EMFOR_IP, EMFOR_PORT), so an existing plugin cannot be given
# `-H $EMFOR_IP`. This script does that: it runs
#
#   PLUGIN F_host $EMFOR_IP F_port $EMFOR_PORT ARGS...
#
# with `-H` and `-p` as the flags (--host-flag, --port-flag; an empty
# --port-flag leaves the port out), and returns the exit status of the plugin.
# An emfor `exec` check reads 0 as up, 1 and 2 as down (the first line of the
# output is the reason) and any other status as an error; a Nagios plugin says
# 0 OK, 1 WARNING, 2 CRITICAL, 3 UNKNOWN. So WARNING is down for emfor, unless
# --warning up turns that status 1 into 0, and UNKNOWN (3) is an error.
# In emfor's config:
#
#   check = "exec"
#   check_options = { command = ["/usr/local/libexec/emfor-exec.sh",
#       "/usr/local/libexec/nagios/check_http", "-u", "/healthz"] }
set -u

warning=down
hflag=-H
pflag=-p
while [ $# -gt 0 ]; do
    case "$1" in
        --warning) warning=${2:?--warning up|down}; shift 2 ;;
        --host-flag) hflag=$2; shift 2 ;;
        --port-flag) pflag=$2; shift 2 ;;
        --) shift; break ;;
        -*) echo "emfor-exec: unknown option $1" >&2; exit 3 ;;
        *) break ;;
    esac
done
[ $# -ge 1 ] || { echo "emfor-exec: no plugin given" >&2; exit 3; }
[ -n "${EMFOR_IP:-}" ] || { echo "emfor-exec: EMFOR_IP is not set (run it from an emfor exec check)" >&2; exit 3; }
plugin=$1; shift

if [ -n "$pflag" ]; then
    "$plugin" "$hflag" "$EMFOR_IP" "$pflag" "${EMFOR_PORT:-}" "$@"
else
    "$plugin" "$hflag" "$EMFOR_IP" "$@"
fi
rc=$?
if [ "$rc" = 1 ] && [ "$warning" = up ]; then exit 0; fi
exit "$rc"
