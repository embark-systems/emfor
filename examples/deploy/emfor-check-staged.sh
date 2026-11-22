#!/bin/sh
# Check one conf.d file before it replaces the file in place, for the
# validation option of a configuration management tool.
# The recipe is docs/manual/05-configuring.md, "Using a configuration
# management tool".
#
#   emfor-check-staged.sh NAME CANDIDATE
#
# NAME is the name of the file in conf.d (10-web.toml), CANDIDATE is the file
# that is to take its place (the temporary file the tool hands over). The
# script builds a staged copy of the config: the main file, and next to it a
# conf.d with every file of the real one but NAME, and CANDIDATE as NAME. It
# runs emfor check-config on that copy and prints what check-config prints to
# standard error. The exit status is that of check-config: 0 for a valid
# config, 2 for an invalid one.
#
# For the tools:
#   Ansible   validate: /usr/local/sbin/emfor-check-staged.sh 10-web.toml %s
#   Puppet    validate_cmd => '/usr/local/sbin/emfor-check-staged.sh 10-web.toml %'
#   Salt      check_cmd: /usr/local/sbin/emfor-check-staged.sh 10-web.toml
#
# Run as root on the host that runs emfor; check-config runs as the daemon's
# user, so that a plugin the config declares does not run as root. The staged
# copy is owned by that user and does not depend on the caller's umask, as
# check-config refuses files that group or others can write.
# Environment:
#   EMFOR_USER         the user emfor runs as (default "emfor")
#   EMFOR_CONFIG       the main config file (default /etc/emfor/emfor.toml)
#   EMFOR_INCLUDE_DIR  the name of its include_dir, relative to the file
#                      (default "conf.d")
# The script changes nothing but its own temporary directory.
set -eu
umask 022

die() { echo "emfor-check-staged: $*" >&2; exit 1; }
[ "$#" -eq 2 ] || die "usage: emfor-check-staged.sh NAME CANDIDATE"
name=$1
candidate=$2
case $name in */* | "") die "NAME is a file name, not a path: $name" ;; esac
[ -f "$candidate" ] || die "no such file: $candidate"

daemon_user=${EMFOR_USER:-emfor}
config=${EMFOR_CONFIG:-/etc/emfor/emfor.toml}
include=${EMFOR_INCLUDE_DIR:-conf.d}
real_dir=$(dirname "$config")/$include
[ -f "$config" ] || die "no such config file: $config"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
cp -p "$config" "$stage/$(basename "$config")"
mkdir "$stage/$include"
for f in "$real_dir"/*.toml; do
    [ -e "$f" ] || continue
    [ "$(basename "$f")" = "$name" ] && continue
    cp -p "$f" "$stage/$include/"
done
install -m 0644 "$candidate" "$stage/$include/$name"

if [ "$(id -u)" -ne 0 ] || [ "$daemon_user" = root ]; then
    emfor --config "$stage/$(basename "$config")" check-config >&2
elif command -v runuser >/dev/null 2>&1; then
    chown -R "$daemon_user" "$stage"
    runuser -u "$daemon_user" -- emfor --config "$stage/$(basename "$config")" check-config >&2
else
    die "runuser is not available: run this as $daemon_user (EMFOR_USER) or as root with runuser"
fi
