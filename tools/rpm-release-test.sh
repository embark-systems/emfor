#!/bin/sh
# Test the emfor rpm under systemd in a Rocky Linux container, before a release.
#
#   tools/rpm-release-test.sh NEW.rpm [OLD.rpm]
#
# NEW.rpm is the package to release; OLD.rpm is the one of the previous release
# (see RELEASING.md for how to build it). Without OLD.rpm the upgrade checks are
# skipped, and the last line says so. Needs docker. The same script runs on an
# arm64 and an x86-64 host; the rpm must match the host's architecture.
#
# It starts ROCKY_IMAGE (default rockylinux:9) with systemd as PID 1 (the image
# has none, so dnf installs it), then in that container it checks that the rpm
#   - installs, creates the emfor user and group, and is not enabled;
#   - starts, reloads, answers `emfor status --json`, and stops;
#   - verifies clean with `rpm -V`;
#   - is removed (a changed config file stays as .rpmsave, the user and the
#     data stay) and installs again;
# and, with OLD.rpm, that an upgrade
#   - restarts the daemon and keeps the admin's changes to the config file;
#   - keeps the state history across the restart;
# and that a downgrade (rpm -U --oldpackage) starts.
#
# A container is not a real host: this does not test SELinux enforcing, the
# host's own systemd, or any other distribution. Nothing here is run by the CI
# or by tools/docker-test.sh.
set -eu

image=${ROCKY_IMAGE:-rockylinux:9}

if [ "${1:-}" = "--inside" ]; then
    inside=1
else
    inside=0
fi

# ---------------------------------------------------------------- outside
if [ "$inside" = 0 ]; then
    cd "$(dirname "$0")/.."
    new=${1:-}
    old=${2:-}
    if [ -z "$new" ] || [ ! -f "$new" ] || { [ -n "$old" ] && [ ! -f "$old" ]; }; then
        echo "usage: tools/rpm-release-test.sh NEW.rpm [OLD.rpm]" >&2
        exit 2
    fi
    abs() { (cd "$(dirname "$1")" && echo "$PWD/$(basename "$1")"); }
    new=$(abs "$new")
    [ -n "$old" ] && old=$(abs "$old")
    self=$(abs tools/rpm-release-test.sh)

    name=emfor-rpm-test-$$
    cleanup() { docker rm -f "$name" >/dev/null 2>&1 || :; }
    trap cleanup EXIT
    trap 'exit 130' INT TERM

    set -- -v "$new:/rpms/new.rpm:ro" -v "$self:/rpm-test.sh:ro"
    [ -n "$old" ] && set -- "$@" -v "$old:/rpms/old.rpm:ro"
    docker run -d --name "$name" --privileged --cgroupns=host \
        --tmpfs /run --tmpfs /tmp "$@" "$image" \
        sh -c 'dnf -y -q install systemd >/dev/null && exec /sbin/init' >/dev/null

    # Wait for systemd (the install takes a while on a cold image).
    i=0
    until docker exec "$name" systemctl is-system-running --wait >/dev/null 2>&1 \
        || [ "$(docker exec "$name" systemctl is-system-running 2>/dev/null || :)" = degraded ]; do
        i=$((i + 1))
        if [ "$i" -gt 90 ] || [ "$(docker inspect -f '{{.State.Running}}' "$name")" != true ]; then
            echo "FAIL: systemd did not come up in the container" >&2
            docker logs "$name" 2>&1 | tail -20 >&2
            exit 1
        fi
        sleep 2
    done
    docker exec "$name" sh /rpm-test.sh --inside "$([ -n "$old" ] && echo yes || echo no)"
    exit $?
fi

# ----------------------------------------------------------------- inside
have_old=$2
failed=0
ok() { echo "ok: $*"; }
fail() { echo "FAIL: $*"; failed=1; }
check() { # check DESCRIPTION COMMAND...
    desc=$1; shift
    if "$@" >/dev/null 2>&1; then ok "$desc"; else fail "$desc"; fi
}
active() { [ "$(systemctl is-active emfor)" = active ]; }
wait_active() {
    n=0
    until active; do
        n=$((n + 1))
        [ "$n" -gt 20 ] && return 1
        sleep 0.5
    done
}
# The services' history shows once a check has run; the interval is 1s.
wait_history() {
    n=0
    until emfor status --json 2>/dev/null | grep -q 'probe.svc.emfor'; do
        n=$((n + 1))
        [ "$n" -gt 20 ] && return 1
        sleep 0.5
    done
}
# The time of the oldest state change in the history of the first backend.
first_change() {
    emfor status --json 2>/dev/null | tr -d '\n' | sed 's/.*"history"//' \
        | grep -o '"at": *[0-9]*' | head -1
}
main_pid() { systemctl show -p MainPID --value emfor; }

write_service() {
    cat > /etc/emfor/conf.d/10-probe.toml <<'EOF'
[services."probe.svc.emfor"]
check = "exec"
interval = "1s"
timeout = "500ms"
backends = ["127.0.0.1:9"]
check_options = { command = ["/bin/true"] }
EOF
    chown root:root /etc/emfor/conf.d/10-probe.toml
    chmod 644 /etc/emfor/conf.d/10-probe.toml
}

lifecycle() { # the checks on one installed version
    check "emfor group exists" getent group emfor
    check "emfor user exists" getent passwd emfor
    if [ "$(systemctl is-enabled emfor 2>/dev/null || :)" = disabled ]; then
        ok "the service is not enabled by the package"
    else
        fail "the service is not enabled by the package"
    fi
    check "emfor.toml passes check-config" emfor check-config
    write_service
    check "config with a service passes check-config" emfor check-config
    check "enable --now" systemctl enable --now emfor
    if wait_active; then ok "the service is active"; else fail "the service is active"; return; fi
    if wait_history; then ok "emfor status --json shows the service"; else fail "emfor status --json shows the service"; fi
    check "reload" systemctl reload emfor
    if wait_active; then ok "active after reload"; else fail "active after reload"; fi
    check "stop" systemctl stop emfor
    if ! active; then ok "inactive after stop"; else fail "inactive after stop"; fi
    check "disable" systemctl disable emfor
}

echo "== install the new package"
dnf -y -q install /rpms/new.rpm >/dev/null 2>&1 || rpm -i /rpms/new.rpm
check "package is installed" rpm -q emfor
echo "arch: $(rpm -q --qf '%{ARCH}' emfor), version: $(rpm -q --qf '%{VERSION}-%{RELEASE}' emfor), host: $(uname -m)"
lifecycle

echo "== rpm -V"
rm -f /etc/emfor/conf.d/10-probe.toml
verify=$(rpm -V emfor 2>&1 || :)
if [ -z "$verify" ]; then ok "rpm -V reports nothing"; else fail "rpm -V reports: $verify"; fi

echo "== remove and install again"
echo "# changed by the administrator" >> /etc/emfor/emfor.toml
check "rpm -e" rpm -e emfor
if [ ! -e /usr/bin/emfor ] && [ ! -e /usr/lib/systemd/system/emfor.service ]; then ok "binary and unit are gone"; else fail "binary and unit are gone"; fi
if [ -f /etc/emfor/emfor.toml.rpmsave ]; then ok "the changed config stays as .rpmsave"; else fail "the changed config stays as .rpmsave"; fi
check "emfor user stays after removal" getent passwd emfor
check "state directory stays after removal" test -d /var/lib/emfor
rm -f /etc/emfor/emfor.toml.rpmsave
check "install again" rpm -i /rpms/new.rpm
check "package is installed again" rpm -q emfor
check "unit is known after reinstall" systemctl cat emfor

if [ "$have_old" = yes ]; then
    echo "== upgrade from the old package"
    rpm -e emfor >/dev/null 2>&1 || :
    check "install the old package" rpm -i /rpms/old.rpm
    echo "old: $(rpm -q --qf '%{VERSION}-%{RELEASE}' emfor)"
    echo "# changed by the administrator" >> /etc/emfor/emfor.toml
    write_service
    check "enable --now (old)" systemctl enable --now emfor
    if wait_active && wait_history; then ok "the old version runs and has history"; else fail "the old version runs and has history"; fi
    pid_before=$(main_pid)
    n=0
    while [ -z "$(first_change)" ] && [ "$n" -lt 20 ]; do n=$((n + 1)); sleep 0.5; done
    change_before=$(first_change)
    if [ -n "$change_before" ]; then ok "the old version recorded a state change ($change_before)"; else fail "the old version recorded a state change"; fi
    check "rpm -U to the new package" rpm -U --replacepkgs /rpms/new.rpm
    echo "new: $(rpm -q --qf '%{VERSION}-%{RELEASE}' emfor)"
    check "emfor user and group stay" sh -c 'getent passwd emfor && getent group emfor'
    if grep -q 'changed by the administrator' /etc/emfor/emfor.toml; then ok "the administrator's change to emfor.toml stays"; else fail "the administrator's change to emfor.toml stays"; fi
    old_sum=$(rpm -qp --dump /rpms/old.rpm | grep '^/etc/emfor/emfor.toml ' | cut -d' ' -f4)
    new_sum=$(rpm -qp --dump /rpms/new.rpm | grep '^/etc/emfor/emfor.toml ' | cut -d' ' -f4)
    if [ -z "$old_sum" ] || [ -z "$new_sum" ]; then
        fail "emfor.toml is not in the dump of both rpms (old: '$old_sum', new: '$new_sum')"
    elif [ "$old_sum" = "$new_sum" ]; then
        echo "note: the packaged emfor.toml is the same in both, so no .rpmnew is expected (not tested)"
    elif [ -f /etc/emfor/emfor.toml.rpmnew ]; then
        ok "emfor.toml.rpmnew was written (the packaged config changed)"
    else
        fail "emfor.toml.rpmnew was written (the packaged config changed)"
    fi
    if wait_active; then ok "active after the upgrade"; else fail "active after the upgrade"; fi
    pid_after=$(main_pid)
    if [ -n "$pid_after" ] && [ "$pid_after" != 0 ] && [ "$pid_before" != "$pid_after" ]; then
        ok "the daemon was restarted by the upgrade (pid $pid_before -> $pid_after)"
    else
        fail "the daemon was restarted by the upgrade (pid $pid_before -> $pid_after)"
    fi
    if [ -n "$change_before" ] && [ "$(first_change)" = "$change_before" ]; then
        ok "the history kept its oldest state change across the restart"
    else
        fail "the history kept its oldest state change across the restart (was: $change_before, now: $(first_change))"
    fi

    echo "== downgrade"
    check "rpm -U --oldpackage" rpm -U --oldpackage --replacepkgs /rpms/old.rpm
    if wait_active; then ok "active after the downgrade"; else fail "active after the downgrade"; fi
    if wait_history; then ok "the old version answers status --json"; else fail "the old version answers status --json"; fi
else
    echo "upgrade checks skipped: no OLD.rpm given"
fi

if [ "$failed" = 0 ]; then
    echo "all checks passed"
else
    echo "some checks FAILED"
    exit 1
fi
