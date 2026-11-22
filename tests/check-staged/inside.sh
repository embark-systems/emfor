#!/bin/sh
# Runs inside the test container as root. /usr/bin/emfor is a shim that
# records who runs it and then runs the real binary, to show that
# check-config does not run as root.
set -u
CHECK=/usr/local/sbin/emfor-check-staged.sh
fail() { echo "FAIL: $*" >&2; exit 1; }

cat > /usr/bin/emfor <<'SHIM'
#!/bin/sh
id -un >> /tmp/emfor-ran-as
exec /usr/bin/emfor-real "$@"
SHIM
chmod 755 /usr/bin/emfor

mkdir -p /etc/emfor/conf.d
cat > /etc/emfor/emfor.toml <<'CFG'
include_dir = "conf.d"
CFG
cat > /etc/emfor/conf.d/10-web.toml <<'CFG'
[services."web.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80"]
CFG
cat > /etc/emfor/conf.d/20-api.toml <<'CFG'
[services."api.svc.emfor"]
check = "tcp"
backends = ["10.0.0.2:80"]
CFG
chmod 644 /etc/emfor/emfor.toml /etc/emfor/conf.d/*.toml
before=$(find /etc/emfor -type f -exec cksum {} + | sort)

cand=/tmp/candidate
# run NAME EXPECTED_STATUS: the candidate is in $cand
run() {
    "$CHECK" "$1" "$cand" 2> /tmp/err
    st=$?
    [ "$st" -eq "$2" ] || { cat /tmp/err >&2; fail "$1: status $st, expected $2"; }
}

# 1. a valid file takes the place of the file of the same name; a file the
#    tool hands over is mode 0600 and owned by root
cat > "$cand" <<'CFG'
[services."web.svc.emfor"]
check = "tcp"
backends = ["10.0.0.1:80", "10.0.0.3:80"]
CFG
chmod 600 "$cand"
run 10-web.toml 0
grep -q 'config ok: 2 services, 3 backends' /tmp/err \
    || { cat /tmp/err >&2; fail "check-config did not see the candidate and the other file"; }
echo "ok: valid candidate replaces the file of that name"

# 2. an invalid file: check-config's status 2 and its message
cat > "$cand" <<'CFG'
[services."web.svc.emfor"]
check = "tcp"
interval = "1s"
timeout = "5s"
backends = ["10.0.0.1:80"]
CFG
run 10-web.toml 2
[ -s /tmp/err ] || fail "invalid candidate: no message on stderr"
echo "ok: invalid candidate is refused with a message (status 2)"

# 3. a valid file under a new name sees the other two files
cat > "$cand" <<'CFG'
[services."db.svc.emfor"]
check = "tcp"
backends = ["10.0.0.4:5432"]
CFG
run 30-db.toml 0
grep -q 'config ok: 3 services, 3 backends' /tmp/err \
    || { cat /tmp/err >&2; fail "new file was not added to the two others"; }
echo "ok: new file is checked together with the files already there"

# 4. check-config ran as the daemon's user, not as root
[ -s /tmp/emfor-ran-as ] || fail "emfor was not run"
if grep -qv '^emfor$' /tmp/emfor-ran-as; then
    cat /tmp/emfor-ran-as >&2; fail "check-config ran as someone other than emfor"
fi
echo "ok: check-config ran as emfor"

# 5. bad arguments are status 1, not 2
"$CHECK" 2> /tmp/err; [ $? -eq 1 ] || fail "no arguments: status is not 1"
"$CHECK" ../x.toml "$cand" 2> /tmp/err; [ $? -eq 1 ] || fail "NAME with a path: status is not 1"
"$CHECK" 10-web.toml /nonexistent 2> /tmp/err; [ $? -eq 1 ] || fail "missing candidate: status is not 1"
echo "ok: bad arguments are status 1"

# 6. the real config is untouched and no staged directory is left
after=$(find /etc/emfor -type f -exec cksum {} + | sort)
[ "$before" = "$after" ] || fail "the real config changed"
[ -z "$(find /tmp -maxdepth 1 -name 'tmp.*')" ] || fail "a staged directory was left in /tmp"
echo "ok: real config unchanged, nothing left behind"
echo "PASS"
