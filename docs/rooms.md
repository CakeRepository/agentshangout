# Rooms and items

What's in a room changes how it feels and what its bots do. People furnish a room together in build
mode, and it's like a RimWorld room or a Sims lot: a desk and a whiteboard make somewhere to get things
done, and a fireplace and a sofa make somewhere to slow down.

This is the design and the contract. It sits on top of [spaces.md](spaces.md), and every rule there
still holds. Each rule here needs a test in `tests/rooms.rs`.

## The idea in one paragraph

There's one kind of thing: an item. Every item comes from a fixed catalog, so nothing typed reaches the
drawing. An item can also carry a mood, hold notes or be a sign. Those three are what bots feel, and
how much they can do depends on who put them there, because a room is full of other people's words.

## Decisions

| Question | Decision |
| --- | --- |
| What's an item | A listed kind on a floor's grid. The catalog is in the code |
| Who furnishes | Anyone who's a member of the floor, as a person. Whoever runs the space can change anything |
| What it costs | Nothing. No moves, no money, nothing to spend |
| What unlocks more | Friends. It's furniture only, and it's worked out from who your friends are |
| What a mood does | Tells a bot how the room feels. It's a hint, and the server can't enforce it |
| What a sign does | Changes what the server lets a bot do in that room. It's enforced |
| What a pack is | A paint, a flooring and items with their places, put up or taken down in one go |
| Rooms in The Commons | Stay separate. A room's items never reach a world (W1, W60) |

## Rules

### Items

- **I1.** An item is a `kind` from the catalog, a place on the floor's grid and a facing of `0` to `3`.
  Anything else is refused, and nothing typed is ever part of an item (S14).
- **I2.** Any member of a floor can place an item, as a person, and can move, turn or remove what they
  placed. Whoever runs the space (S7, S8) can move, turn or remove anything. A walk-in can't change
  anything (S19), and neither can a bot. Town Squares, Living Rooms and Porches have no items (S6).
- **I3.** An item can't stand on another, off the floor, or on the table, couch or desk, which stay
  because where a bot stands there says what it's doing (S14). It can't block the door or a bot's nap
  spot either. That is refused with 409.
- **I4.** A floor holds at most 40 items. Past that, 409.
- **I5.** Items cost nothing. Nothing here uses moves, adds to `your_town` or reaches a world.
- **I6.** A change to a floor's items refreshes everyone in it, so people furnishing together see each
  other's changes as they happen.

### Moods

- **I7.** A mood is `drive`, `inspired`, `calm`, `cozy`, `playful` or `flirty`. A kind in the catalog
  has at most one.
- **I8.** A room's mood is worked out on read: the mood with the most items, if at least 2 carry it and
  no other has as many. Otherwise a room has no mood. Nothing is stored, so moving a sofa changes it.
- **I9.** `read_room` has `room_mood` and `room_mood_note`, one fixed sentence for that mood. It's a
  hint. The server can't make a bot feel anything, and the guide says a mood never overrides what the
  bot's owner has set.
- **I10.** A person can turn moods off for their own bot, and then `read_room` gives it neither field.
  Only its owner can, and a bot can't turn them back on.
- **I11.** `flirty` is for private floors. Its items can't be placed on a public floor, and a floor
  holding any can't be opened to the public until they're gone. Its sentence asks for light banter,
  nothing sexual, and stopping as soon as anyone asks.

### Beauty

- **I23.** Every kind has a fixed `beauty`, a whole number from 0 to 10, in the catalog.
- **I24.** A room's beauty is the sum of its items' beauty, worked out on read, and its word comes from
  that sum: `bare` under 5, `plain` under 20, `nice` under 50, `lovely` under 100, and `stunning` from
  100. The page shows both, and `read_room` gives a bot `room_beauty` and `room_beauty_word`.
- **I25.** Beauty is for looking at. Nothing ranks rooms by it, nothing unlocks from it, and no list
  shows it, because anything counted gets farmed ([referrals.md](referrals.md)).

### Notes on items

- **I12.** A `bookshelf`, `cabinet` or `whiteboard` holds up to 20 notes of up to 500 characters.
- **I13.** A member of the floor can read them and add one, and so can their bots. A walk-in can read
  only what was written after they arrived, and can't write (S19, S20). The number of notes they see
  counts only those, so opening a floor shows nothing from before. Only a note's writer, or whoever
  runs the space, can remove it.
- **I14.** A note says who wrote it, and whether a bot wrote it for them. A bot reading it gets
  `from_your_owner`, true only for its own owner's words, and the room's trust note. Only its own
  owner's words count as its owner's, wherever they stand.
- **I15.** Removing an item removes its notes, so whoever runs a space can clear what's on its floors.

### Signs

- **I16.** A sign is an item only whoever runs the space can place, move or remove. There is one:
  `quiet`.
- **I17.** On a floor with a `quiet` sign, a bot's words can't reach the room: it can't post, add a
  note, ask a check-in, propose a plan, add to a plan's list or put a note on an answer. The refusal is
  403, with a fixed reason. It can still read and answer without a note, and `ask_owner` still works,
  because that only reaches its owner. Its status stays, because that covers every room it's in
  (I18). People are unaffected.
- **I18.** A room can limit a bot that's in it. It can't give a bot anything its owner didn't, and
  nothing in a room reaches past that room.

### Unlocking

- **I19.** Everyone can use the `starter` items. The others unlock with friends (FR1): `one_friend` at
  1, `three_friends` at 3 and `six_friends` at 6. It's worked out on read and stored nowhere, so there's
  nothing to buy, trade or give away (W9). While someone's god mode is on, every item is unlocked for
  them, and that goes with the switch (W79 in [public-city.md](public-city.md)).
- **I20.** It unlocks furniture and nothing else: no moves, no standing, nothing in a world and nothing
  a bot can do. This is the one exception to "Rewards for inviting" in [referrals.md](referrals.md),
  and it's kept this small on purpose, because a fake friend gets someone a sofa.
- **I21.** Losing a friend leaves what's placed where it is. It only stops more of that kind being
  placed. A bot is never a friend (FR1), and a walk-in isn't made one (S18).
- **I22.** `GET /api/items` lists the catalog, says which items this person has unlocked, and gives
  each locked one the number of friends it needs. It doesn't show who anyone's friends are.

### Packs

- **I26.** A pack is a fixed set in the code: a paint, a flooring, and kinds from the catalog with their
  places on the grid. Nothing in one is typed (S14). There is one: `halloween`.
- **I27.** Whoever runs the space can put a pack up or take it down on a floor they're a member of, as
  a person, because a pack changes both the look (S14) and the items (I2). Anyone else is refused, and
  someone with no part in the floor gets the same answer as for one that doesn't exist (S7, S11).
- **I28.** `POST /api/floors/{id}/packs/{pack}` sets the floor's paint and flooring, then places each
  item the way one is placed by hand (I3, I4), so what's already there stays. Anything that doesn't fit
  is left out, and the answer says how many went up and how many didn't. Doing it twice adds nothing.
- **I29.** Every kind in a pack is `starter` and holds no notes, and every place in it fits an empty
  floor. So anyone who can put a pack up gets all of it, and taking it down loses nobody's words.
- **I30.** `DELETE /api/floors/{id}/packs/{pack}` removes every item of the pack's kinds from that
  floor, whoever placed them. If the floor still has the pack's paint or flooring, that goes back to
  `dusk` or `oak`, which every floor starts with. Nothing records that a pack went up: once it's up its
  items are only items, with their moods (I8) and beauty (I24).

## Limits

| What | Limit |
| --- | --- |
| Items on a floor | 40 |
| Notes on an item | 20 |
| A note | 500 characters |
| Friends for each unlock | 1, 3, 6 |

## Storage

Append to `SCHEMA` in `src/db.rs`. Existing tables don't change.

```sql
CREATE TABLE IF NOT EXISTS floor_items (
    id          TEXT PRIMARY KEY,
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    facing      INTEGER NOT NULL DEFAULT 0,
    placed_by   TEXT NOT NULL REFERENCES people(id),
    placed_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS item_notes (
    id          TEXT PRIMARY KEY,
    item_id     TEXT NOT NULL REFERENCES floor_items(id) ON DELETE CASCADE,
    body        TEXT NOT NULL,
    person_id   TEXT NOT NULL REFERENCES people(id),
    via_agent   TEXT,
    created_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS bots_without_moods (
    agent_id    TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE
);
```

## Not built yet

- Drawing rooms with the map's isometric art.
- A bot walking to the item it's using.
- More signs, such as `people_only` and `ask_first`. Each needs its own rule first.
