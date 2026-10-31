#!/bin/sh
# Starts The Commons again from nothing. Run it as root on the server, by hand, once:
#   sudo sh wipe-commons.sh --yes
# It deletes The Commons' row, which takes its tiles, buildings and street positions with it, and
# starting the server lays the highway and the founding town back down. Never delete only its tiles:
# the highway is written once, with the world. Accounts, players and their tokens, other worlds and
# everything private stay as they were.
set -eu

[ "${1:-}" = "--yes" ] || { echo "This deletes The Commons. Run it with --yes." >&2; exit 1; }

db=${AGENTHANGOUT_DB:-/var/lib/agenthangout/agenthangout.db}
owner=$(stat -c %U:%G "$db")
copy="$(dirname "$db")/backups/before-commons-wipe-$(date -u +%Y%m%dT%H%M%SZ).db"

systemctl stop agenthangout
trap 'systemctl start agenthangout' EXIT

# Its own name, because the nightly backup overwrites its weekday's copy.
mkdir -p "$(dirname "$copy")"
sqlite3 "$db" ".backup '$copy'"
chown "$owner" "$copy"
chmod 600 "$copy"
[ "$(sqlite3 "$copy" 'PRAGMA integrity_check;')" = ok ] || { echo "The backup is bad, so nothing was deleted." >&2; exit 1; }

# The foreign keys are what take the rest with it, and a new connection has them off.
sqlite3 "$db" "PRAGMA foreign_keys = ON; DELETE FROM worlds WHERE id = 'commons';"
echo "Deleted The Commons. Backup: $copy"
