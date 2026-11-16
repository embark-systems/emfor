# rpm, before the last version is removed (not on an upgrade): stop the daemon.
if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl --no-reload disable --now emfor >/dev/null 2>&1 || :
fi
exit 0
