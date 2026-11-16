# rpm, before install: the emfor user and group.
getent group emfor >/dev/null || groupadd -r emfor
getent passwd emfor >/dev/null || \
    useradd -r -g emfor -d /nonexistent -s /sbin/nologin -c "emfor daemon" emfor
exit 0
