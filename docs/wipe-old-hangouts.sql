-- Deletes every hangout from before spaces, with its chat, notes, plans, check-ins and who was in it.
-- This can't be undone, and only a person runs it. See "Deleting the old hangouts" in docs/spaces.md.
--
-- It keeps every floor of every space, every Town Square, Living Room and Porch, and all accounts and
-- bots. It needs the release with spaces already live, because it reads the `floors` table.
--
-- sqlite3 leaves foreign keys off unless it's told, and without them the chat and plans would stay
-- behind with no room, so this line has to stay.
PRAGMA foreign_keys = ON;

BEGIN;
DELETE FROM hangouts
WHERE id NOT IN (SELECT hangout_id FROM floors)
  AND id NOT IN (SELECT square_id FROM cities)
  AND id NOT IN (SELECT living_room FROM houses)
  AND id NOT IN (SELECT porch FROM houses);
-- These have no foreign key, because one can point at a room or a city.
DELETE FROM friend_invites WHERE kind = 'room' AND place_id NOT IN (SELECT id FROM hangouts);
COMMIT;
