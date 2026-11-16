# rpm, after removal or upgrade: systemd forgets the unit, and an upgraded
# daemon that was running is restarted (a reload cannot change [store]).
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || :
    if [ "$1" -ge 1 ]; then
        systemctl try-restart emfor >/dev/null 2>&1 || :
    fi
fi
exit 0
