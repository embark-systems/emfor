#!/bin/sh
# Usage: tests/pf/validate.sh      (any system with sh, awk, sed, sort; no pf)
#
# Checks that examples/pf/emfor-pf-table.sh gives pfctl only addresses, with a
# fake `drill` (the SRV answer is a file) and a fake `pfctl` (it records what
# it is asked to do). The run on a real FreeBSD with pf is tests/pf/run.sh.
set -u

root="$(cd "$(dirname "$0")/../.." && pwd)"
UPDATER="$root/examples/pf/emfor-pf-table.sh"
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
mkdir "$W/bin"

cat > "$W/bin/drill" <<'DRILL'
#!/bin/sh
cat "$FAKE_SRV"
DRILL
cat > "$W/bin/pfctl" <<'PFCTL'
#!/bin/sh
# pfctl -t TABLE -T show | flush | replace ADDR...
echo "$*" >> "$FAKE_LOG"
case "$4" in
    show) cat "$FAKE_TABLE" 2>/dev/null ;;
    flush) : > "$FAKE_TABLE" ;;
    replace)
        if [ -n "${FAKE_PFCTL_FAIL:-}" ]; then
            echo "pfctl: Invalid argument" >&2
            echo "second line" >&2
            exit 1
        fi
        shift 4
        printf '%s\n' "$@" > "$FAKE_TABLE" ;;
esac
PFCTL
chmod +x "$W/bin/drill" "$W/bin/pfctl"

fail() { echo "FAIL: $*" >&2; exit 1; }

# An SRV answer: one line a target "<name> <priority>".
answer() {
    {
        echo ';; ->>HEADER<<- opcode: QUERY, rcode: NOERROR, id: 1'
        while read -r target prio; do
            echo "_x._tcp.api.svc.emfor. 5 IN SRV $prio 1 8080 $target.api.svc.emfor."
        done
    } > "$W/srv"
}
servfail() { echo ';; ->>HEADER<<- opcode: QUERY, rcode: SERVFAIL, id: 1' > "$W/srv"; }

# run [VAR=value ...] -- args : the updater with the fakes in front of PATH
run() {
    : > "$W/log"; : > "$W/table"
    env PATH="$W/bin:$PATH" FAKE_SRV="$W/srv" FAKE_LOG="$W/log" FAKE_TABLE="$W/table" "$@" \
        > "$W/out" 2> "$W/err"
    echo $? > "$W/rc"
}
rc() { cat "$W/rc"; }

# 1. a valid and an invalid mix: only the valid ones reach pfctl
printf '%s\n' '10-0-0-11 0' '10-0-0-300 0' 'evil-name 0' '2001-db8-0-0-0-0-0-1 0' \
    '1-2-3 0' 'g001-db8-0-0-0-0-0-1 0' | answer
run sh "$UPDATER" once api api
[ "$(rc)" = 0 ] || fail "mix: exit $(rc): $(cat "$W/err")"
grep -q -- '-T replace 10.0.0.11 2001:db8:0:0:0:0:0:1$' "$W/log" || fail "mix: pfctl got: $(cat "$W/log")"
[ "$(grep -c 'ignoring SRV target' "$W/err")" = 4 ] || fail "mix: warnings: $(cat "$W/err")"
grep -q 'ignoring SRV target 10-0-0-300.api.svc.emfor.: not an address' "$W/err" || fail "mix: no warning for 10-0-0-300"

# 2. only valid addresses: no warning, the same arguments as before
printf '%s\n' '10-0-0-12 0' '10-0-0-11 0' | answer
run sh "$UPDATER" once api api
grep -q -- '-T replace 10.0.0.11 10.0.0.12$' "$W/log" || fail "valid: pfctl got: $(cat "$W/log")"
grep -q ignoring "$W/err" && fail "valid: warning: $(cat "$W/err")"

# 3. a target that is not an address is not counted for the priority
printf '%s\n' 'evil-name 0' '10-0-0-12 5' | answer
run sh "$UPDATER" once api api
grep -q -- '-T replace 10.0.0.12$' "$W/log" || fail "priority: pfctl got: $(cat "$W/log")"

# 4. EMPTY and the table name are checked at the start
servfail
run EMPTY='127.0.0.1; reboot' sh "$UPDATER" once api api
[ "$(rc)" != 0 ] && grep -q 'EMPTY is not' "$W/err" || fail "bad EMPTY: $(rc) $(cat "$W/err")"
[ ! -s "$W/log" ] || fail "bad EMPTY reached pfctl"
run EMPTY=127.0.0.1 sh "$UPDATER" once api api
grep -q -- '-T replace 127.0.0.1$' "$W/log" || fail "EMPTY: pfctl got: $(cat "$W/log")"
for bad in -T 'a b' 'a;b' 'a*'; do
    run sh "$UPDATER" once api "$bad"
    [ "$(rc)" != 0 ] && grep -q 'not a pf table name' "$W/err" || fail "table '$bad': $(rc) $(cat "$W/err")"
    [ ! -s "$W/log" ] || fail "table '$bad' reached pfctl"
done

# 5. pfctl fails: the first line of its message is in ours
printf '%s\n' '10-0-0-11 0' | answer
run FAKE_PFCTL_FAIL=1 sh "$UPDATER" once api api
[ "$(rc)" != 0 ] && grep -q 'pfctl could not replace api: pfctl: Invalid argument' "$W/err" || fail "pfctl failure: $(rc) $(cat "$W/err")"
grep -q 'second line' "$W/err" && fail "more than the first line was shown"

echo "ok: tests/pf/validate.sh"
