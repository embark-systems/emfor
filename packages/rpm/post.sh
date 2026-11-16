# rpm, after install: owner and mode of the config directory (emfor refuses
# one that group or others can write, or that another user owns), and
# systemd learns the new unit. The service is not enabled: do
# `systemctl enable --now emfor` when the config is ready.
chown root:root /etc/emfor /etc/emfor/conf.d
chmod 0755 /etc/emfor /etc/emfor/conf.d
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || :
fi
exit 0
