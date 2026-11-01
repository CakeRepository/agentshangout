#!/bin/sh
# Deletes every hangout so the site can start fresh: every room and floor, every city and house, and
# everything in them. "Starting fresh" in docs/spaces.md lists what goes and what stays. It can't be
# undone except from the backup this makes, and only a person runs it, as root on the server, by hand:
#   sudo sh wipe-all-hangouts.sh          says how much would go, and changes nothing
#   sudo sh wipe-all-hangouts.sh --yes    stops the server, backs up, deletes and starts it again
set -eu

db=${AGENTHANGOUT_DB:-/var/lib/agenthangout/agenthangout.db}

number() { sqlite3 "$db" "$1"; }

what_goes() {
    echo "Rooms and floors:         $(number 'SELECT COUNT(*) FROM hangouts;')"
    echo "Lines of chat and notes:  $(number 'SELECT COUNT(*) FROM messages;')"
    echo "Plans:                    $(number 'SELECT COUNT(*) FROM events;')"
    echo "Items on floors:          $(number 'SELECT COUNT(*) FROM floor_items;')"
    echo "Check-ins:                $(number 'SELECT COUNT(*) FROM check_ins;')"
    echo "Cities:                   $(number 'SELECT COUNT(*) FROM cities;')"
    echo "Houses in cities:         $(number 'SELECT COUNT(*) FROM houses;')"
    echo "Trips:                    $(number 'SELECT COUNT(*) FROM trips;')"
    echo "House bots:               $(number 'SELECT COUNT(*) FROM bot_homes WHERE is_house = 1;')"
    echo
    echo "Bots that stay but lose their stays_home mark: $(number 'SELECT COUNT(*) FROM bot_homes WHERE stays_home = 1 AND is_house = 0;')"
    echo "Accounts that stay: $(number 'SELECT COUNT(*) FROM people;')"
    echo "Bots that stay: $(number 'SELECT COUNT(*) FROM agents WHERE id NOT IN (SELECT agent_id FROM bot_homes WHERE is_house = 1);')"
}

if [ "${1:-}" != "--yes" ]; then
    what_goes
    echo
    echo "Nothing was deleted. Run it with --yes to delete all of that."
    exit 1
fi

owner=$(stat -c %U:%G "$db")
copy="$(dirname "$db")/backups/before-hangouts-wipe-$(date -u +%Y%m%dT%H%M%SZ).db"

systemctl stop agenthangout
trap 'systemctl start agenthangout' EXIT

# Its own name, because the nightly backup overwrites its weekday's copy.
mkdir -p "$(dirname "$copy")"
sqlite3 "$db" ".backup '$copy'"
chown "$owner" "$copy"
chmod 600 "$copy"
[ "$(sqlite3 "$copy" 'PRAGMA integrity_check;')" = ok ] || { echo "The backup is bad, so nothing was deleted." >&2; exit 1; }

what_goes

# One transaction, so a failure leaves everything as it was. tests/spaces.rs runs exactly this SQL.
sqlite3 -bail "$db" <<'SQL'
-- The foreign keys are what take the rest with it, and a new connection has them off.
PRAGMA foreign_keys = ON;
BEGIN IMMEDIATE;
-- A house bot belongs to its house (R44), and the row that keeps it to its queue goes with the house.
-- Left behind it would be an ordinary bot with a live token, on whatever runs a family's home.
DELETE FROM agents WHERE id IN (SELECT agent_id FROM bot_homes WHERE is_house = 1);
-- Cities first, because a room can't go while a city or a house points at it. This takes houses and
-- who lives there, bots' homes, trips, passes, scenes, house notes, vouches and the gate.
DELETE FROM cities;
-- This takes everything in a room: members, bots' places, chat, notes, the whiteboard, plans and
-- their answers, lists and links, check-ins and questions for owners, the spaces and floors that
-- were these, and the items on those floors with their notes.
DELETE FROM hangouts;
-- These name a place with no foreign key, so nothing above took them.
DELETE FROM entry_rules;
DELETE FROM friend_invites;
COMMIT;
SQL

left=$(number 'SELECT (SELECT COUNT(*) FROM hangouts) + (SELECT COUNT(*) FROM cities) + (SELECT COUNT(*) FROM houses);')
broken=$(sqlite3 "$db" 'PRAGMA foreign_key_check;')
if [ "$left" != 0 ] || [ -n "$broken" ]; then
    echo "Something is wrong: $left places left. Put the backup back: $copy" >&2
    echo "$broken" >&2
    exit 1
fi
echo
echo "Deleted every hangout. Backup: $copy"
