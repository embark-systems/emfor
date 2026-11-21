#!/bin/sh
# Set the members of this host's peer group from a list, for a group whose
# hosts come and go (an autoscaling group, a managed instance group).
# The recipe is docs/manual/08-operations.md, "Peer members from a cloud API".
#
#   some-command-that-lists-the-hosts | emfor-peers-sync.sh FILE
#
# Standard input is one member address per line, as emfor writes it in
# [peers] members: IPv4:port or [IPv6]:port, this host included. FILE is a
# conf.d file of this host that holds only the [peers] members, for example
# /etc/emfor/conf.d/50-peers.toml; the rest of [peers] (listen, secret_file)
# stays in the main file, and no other file may set members, because the
# last file that sets it replaces the list.
#
# The script changes nothing when the list is the one FILE already holds, or
# when it is not a list it will write: fewer than 2 or more than 64
# addresses (an empty answer of the cloud API is the usual cause: keep the
# old group), or a line that is not an address. Otherwise it writes the new
# list to FILE.new, moves it over FILE, runs emfor check-config, puts the old
# FILE back (or removes the new one when there was none) if check-config
# refuses, and only then reloads emfor. check-config also refuses a list
# that does not contain this host's own listen address.
#
# Run as root on the host that runs emfor (check-config run as root accepts
# only conf.d files owned by root). Environment:
#   EMFOR_RELOAD   how to reload (default "systemctl reload emfor"; FreeBSD:
#                  "service emfor reload")
#   EMFOR_CONFIG   the config file, passed as --config to emfor (default: the
#                  path built into emfor)
# Run it from one timer or one cron job per host, not from two at once.
set -eu

die() { echo "emfor-peers-sync: $*" >&2; exit 1; }
reload=${EMFOR_RELOAD:-systemctl reload emfor}
emfor_cmd() { if [ -n "${EMFOR_CONFIG:-}" ]; then c=$1; shift; emfor "$c" --config "$EMFOR_CONFIG" "$@"; else emfor "$@"; fi; }

file=${1:?FILE}
case "$file" in *.toml) ;; *) die "$file does not end in .toml, so emfor would not read it" ;; esac

# Sorted, so that the same hosts always give the same file and no reload.
list=$(grep -v '^[[:space:]]*$' | sort -u || true)
[ -n "$list" ] || die "no addresses on standard input; $file is unchanged"

# Only addresses go into the file: the list can come from tags or names that
# other people can set, and a quote or a line feed in it would be TOML.
bad=$(printf '%s\n' "$list" |
    grep -Ev '^([0-9]{1,3}(\.[0-9]{1,3}){3}|\[[0-9A-Fa-f:.]+\]):[0-9]{1,5}$' || true)
[ -z "$bad" ] || die "not an address: $(printf '%s' "$bad" | head -n 1); $file is unchanged"
n=$(printf '%s\n' "$list" | wc -l | tr -d ' ')
[ "$n" -ge 2 ] && [ "$n" -le 64 ] || die "$n addresses, emfor takes 2 to 64; $file is unchanged"

{
    echo "# Written by emfor-peers-sync.sh; the next run replaces it."
    echo "[peers]"
    echo "members = ["
    printf '%s\n' "$list" | sed 's/.*/  "&",/'
    echo "]"
} > "$file.new"
chmod 644 "$file.new"
if [ -f "$file" ] && cmp -s "$file" "$file.new"; then
    rm -f "$file.new"
    exit 0
fi
# A file that does not end in .toml is not read by emfor, so the temporary
# files are never loaded.
if [ -f "$file" ]; then cp -p "$file" "$file.old"; fi
mv "$file.new" "$file"
if ! emfor_cmd check-config >/dev/null; then
    if [ -f "$file.old" ]; then mv "$file.old" "$file"; else rm -f "$file"; fi
    die "check-config refuses the new list; $file is as it was"
fi
rm -f "$file.old"
sh -c "$reload"
