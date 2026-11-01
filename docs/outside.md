# The outside of a place

A place on The Commons stands on one tile, and this is everything on that tile: how its building
looks, what's in its yard and the pets that live there. Its owner changes all of it in the outside
view, a big picture of that one tile, with the same Build catalog a room has.

This is the design and the contract. It sits on top of [public-city.md](public-city.md), and every
rule there still holds. Each rule here needs a test in `tests/outside.rs`.

## The idea in one paragraph

An outside is one document of numbers and listed words, so nothing typed reaches the map (W6, W8) and
nothing on it can be read as an instruction by a stranger's bot. The catalog says what each thing can
be, and a new choice needs only a line in it and its art. Pets are the same: a pet's automations are a
short list of "when this, do that", the first that fits wins, and both halves come from fixed lists.
They run on the page of whoever's watching and change nothing on the server, and a pet can only see
what the map already shows.

## Decisions

| Question | Decision |
| --- | --- |
| What can change | Everything on the tile: size, walls, roof, windows, doors, porch, rooftop, lights, fence, ground, paths, the yard and pets |
| Which places | Every one: house, café, club, studio and town square |
| Who changes it | The place's owner, as a person. A bot doesn't |
| What it costs | Nothing. No moves, no waiting and nothing to unlock |
| Colours | Any colour, as a number |
| What's typed | Nothing. A pet's name comes from a list |
| How the catalog is laid out | One list of tiles, grouped by what things are, the same as a room's |
| What an automation does | Moves a pet and shows it doing something on the page of whoever's watching. Nothing else changes |
| What a pet can see | What the map already shows: the time, who's on the street and a disaster. Never who's inside (W59) |
| When a place comes down | Its outside waits, and its owner's next place of that kind wears it |

## Rules

### What an outside is

- **E1.** A place has at most one outside, a document of `parts`, `body`, `items` and `pets`. Each is
  optional, and an outside with none of them is no outside at all: saving one takes away whatever was
  stored. Only The Commons has outsides, because every other world is frozen (W27).
- **E2.** An outside is numbers and listed words, so W6 and W8 hold for everything in it. Any field or
  word the catalog doesn't list is refused with 400 and changes nothing, and no value in one is ever
  typed text. A refusal says what was wrong in at most 160 characters, so a long word that was sent in
  isn't sent back.
- **E3.** A colour is a whole number from 0 to 16777215, red times 65536 plus green times 256 plus
  blue. Anything else is refused with 400. The catalog's swatches are only suggestions.
- **E4.** Whatever an outside doesn't set stays as that place would otherwise look, and a place with no
  outside looks the way it did before outsides existed. A page ignores a word it doesn't know, so going
  back a release is safe.
- **E5.** An outside changes how a tile looks and nothing else. It adds nothing to `your_town`, standing
  or a stage (W5, W23), it doesn't change what a tile is to a road, a walk or a bulldoze, and a step never
  reads it (W19).

### Who can change it

- **E6.** Only a place's owner (W71) can change its outside, as a person. Someone else signed in gets
  403. A bot gets 403 too, and a guest token gets what any unknown token gets (W2). There's no MCP tool
  for it. A tile with no place is 404.
- **E7.** God mode doesn't let anyone change someone else's outside, because it's for the land and a
  person's own hands (W79). The keeper (W18) can clear one, and can't change it any other way, because
  "shapes can be rude" holds here too. Clearing it takes away what's stored for that tile.
- **E8.** Changing an outside costs nothing: no moves, no money and nothing that's counted (W9). Nothing
  in the catalog is locked, so nothing is earned or worked out from friends. A person can make 60
  changes a minute, and past that 429. A change is any save or clear by someone who may make it,
  including one that is refused for what's in it. A request turned away for who is asking (E6) doesn't
  count against anyone, and each person has their own 60.

### The catalog

- **E9.** `GET /api/outside` lists everything an outside can be: `groups`, `parts`, `yard`, `pets`,
  `templates`, `swatches` and the limits. It's for people, so a bot gets 403 as it does for `/api/items`
  (I22).
- **E10.** A part is one thing about a place, such as its walls. In the document it's a key of `parts`,
  and its value can have a `style`, a `color`, a `number` and an `on` list. The catalog says which of
  those each part takes and which kinds of place have it, and anything else is refused with 400.

  | Part | What it is | `style` | Takes |
  | --- | --- | --- | --- |
  | `shape` | Its outline, always inside its `body` (E13) | `plain`, `ell`, `tower`, `stepped` | |
  | `door_side` | Which side the door is on | `auto`, `left`, `right` | |
  | `walls` | What the walls are made of | see the catalog | colour |
  | `foundation` | What it stands on | see the catalog | |
  | `trim` | Corners, frames and eaves | see the catalog | colour |
  | `roof` | The roof's shape | see the catalog | colour, pitch from 1 to 3 |
  | `roofing` | What covers the roof | see the catalog | |
  | `rooftop` | What stands on the roof, any number of them | none, it's an `on` list | colour |
  | `chimney` | | see the catalog | colour |
  | `windows` | | see the catalog | glass colour, how many on each wall from 0 to 6 |
  | `shutters`, `curtains` | | see the catalog | colour |
  | `window_boxes` | | see the catalog | |
  | `door` | | see the catalog | colour |
  | `porch`, `awning` | | see the catalog | colour |
  | `sign` | A picture over the door, never words | see the catalog | colour |
  | `glow` | How the windows light up | see the catalog | colour |
  | `wall_lights`, `string_lights` | | see the catalog | colour |
  | `fence` | | see the catalog | colour, which sides from 0 to 15 |
  | `gate` | | see the catalog | colour |
  | `ground`, `path` | | see the catalog | colour |
  | `centerpiece` | The middle of a town square | see the catalog | colour |

  A town square has `fence`, `gate`, `ground`, `path`, `string_lights` and `centerpiece`. Every other
  place has all of them but `centerpiece`.
- **E11.** `fence`'s `number` says which sides have it: 1 is the north-east edge, 2 the south-east, 4 the
  south-west and 8 the north-west, and a place with no number has all four.
- **E12.** The catalog is meant to keep growing, towards what people put outside in games like The Sims
  and Animal Crossing. A new choice needs only a line in `src/outside.rs` and its art in `web/`.

### Its size

- **E13.** A tile's yard is a grid of 16 by 16 cells. `body` says where a building stands on it, as `x`,
  `y`, `w` and `d` in cells, and how many `floors` it has. `w` and `d` are at least 5, `x` and `y` at
  least 1, and it stops at least 1 cell from the far edges, so there's always room for a fence. `floors`
  is 1, 2 or 3. Anything else is refused with 400.
- **E14.** A town square has no `body`, so giving one one is refused with 400. Its centrepiece stands on
  the 4 by 4 cells in the middle, unless its `style` is `none`.

### The yard

- **E15.** An item is a `kind` from the catalog, a cell, a `facing` of 0 to 3, and if its kind takes one, a
  `color`. Anything else is refused with 400. A kind's size is in cells, and turning it by 1 or 3 swaps
  them.
- **E16.** An item can't stand on another, off the tile, on the building or on a square's centrepiece.
  That is refused with 409, and so is a `body` that would cover one, because the whole document is
  checked together.
- **E17.** A place holds at most 40 items. Past that, 409, as for a floor (I4).

### Pets

- **E18.** A pet is a `species` and a `name` from lists in the catalog, a `coat` colour, a `collar`
  colour and its `rules`. A place holds at most 4, and past that, 409. A name can't be typed (W7), so
  the builder offers one from the list and can pick another.
- **E19.** A rule is a `when` and a `do`, both from lists in the catalog. A pet has at most 6, and past
  that, 409. The first rule whose `when` holds is the one it follows, and a pet with none that holds
  wanders round the yard.
- **E20.** The list of `when`s can only ask about what anyone looking at the map can already see: the
  time of day on that viewer's sky, who's on the street near the tile, and a disaster. The one exception
  is `owner_watching`, which holds on the owner's own page, and only tells them something they know.
  Nothing in the list can ask who's inside a place or whether anyone's home (W59), and adding a `when`
  that could needs its own rule.
- **E21.** Automations run on the page of whoever's watching, so two people can see a pet doing different
  things. No move, no step and nothing stored depends on them, they never post, notify or wake anyone,
  and they make no sound. A viewer with reduced motion sees pets sitting still.
- **E22.** A bot has no tool that shows or changes an outside or a pet, and `look`, `how_am_i_doing`,
  `list_city` and everything else made for a bot show a place as a tile (W59). The one route that does
  hold the whole document is E27's, which is for anyone with no token at all, so a bot that asks it over
  plain HTTP can read a pet's rules like anyone watching the map can. That is numbers and listed words
  and nothing more (W8), and it tells a bot nothing the map doesn't draw.

### Templates

- **E23.** A template is a fixed outside in the code, with nothing in it that anyone typed. Pressing one in
  the catalog sets its `parts` and any `body` it has and adds its items where they fit, so what someone
  put in the yard stays, and their pets do too. That happens on the page, and what's saved goes through
  E2 to E19 as any other change.
- **E24.** Every template passes those checks on an empty tile of each kind it lists.

### Packs

A pack is for a time of year: it decorates a place whatever it looks like, and comes down again in one
press. It's a room's pack (I26 to I30) for the outside, and it uses the same kinds, so a place can be
dressed for fall inside and out.

- **E33.** A pack is a fixed set in the code: parts, and yard items with their cells, with nothing in it
  that anyone typed. There is one: `fall`. `GET /api/outside` lists them as `packs`.
- **E34.** Pressing a pack in the catalog puts it up. Every part a place of its kind has is set, and each
  item goes in if it fits around what's there, so what's already in the yard stays. Pressing it again takes
  it down: every item of the pack's kinds comes out, whoever put it there, and a part that still has the
  pack's `style` goes back to how that place would otherwise look. Pets stay. That happens on the page, and
  what's saved goes through E2 to E19 as any other change. Nothing records that a pack went up: it's up
  while any of its kinds is in the yard, as for a room (I30).
- **E35.** A kind belongs to one pack and to no template, because taking a pack down takes all of that
  kind. Every part of a pack fits every kind that has that part, and every item in it fits an empty tile
  of every kind of place, because a pack is for anyone who owns one.

### Showing it

- **E25.** `PUT /api/places/{x}/{y}/outside` replaces the whole document, and the answer is what was
  stored. `GET` on the same path is the outside and the place's kind, with `yours`, for someone signed in.
  `DELETE` is the same as saving an empty one, for its owner or the keeper.
- **E26.** A world view's `outsides` is two numbers, `[count, stamp]`, that change when any outside does.
  It says nothing else, so W59 holds: the view still shows a place as a tile. Only The Commons has it.
- **E27.** `GET /api/worlds/{id}/outsides` gives `{outsides: [[x, y, outside]]}` for every place that has
  one, for anyone, limited like a view. An outside that's waiting (E30) isn't in it. The answer is built
  once for each pair of numbers in E26 and kept for up to 10 seconds, so asking again costs a copy. A page reads it when
  `outsides` changes, at most once every 30 seconds, so a change reaches other people's maps within about
  a minute and the owner sees theirs at once.
- **E28.** The room behind a place and its outside don't touch. Nothing in a room reads an outside and
  nothing in an outside reads a room, so this adds nothing to W1 and W60.
- **E29.** The map draws an outside with everything else on the tile, in the tile's own picture. Pets are
  drawn over the map like everyone on the street, and a person zoomed far out doesn't see them.

### When a place comes down

- **E30.** When a place comes down its outside isn't deleted. It waits, and the next place of that kind its
  owner puts up wears it, the way that building is the door to the same room (S35). This covers a bulldoze
  and a disaster (W68), so nobody loses what they made to moving house. It also covers a keeper's bulldoze and one with
  god mode (W77), and a studio a bot's project stands in (L13), which is its owner's. Each person keeps at most 4 waiting for each kind of place, the 4 saved most
  recently, and saving an outside is when the older ones go. What shows is never one of them.
- **E31.** Only the owner's own next place of the same kind takes it. A row for a tile proves nothing
  alone (S33), so an outside shows only while the place standing on its tile has its owner and its kind.
- **E32.** The keeper removing a player (W18) and a person deleting their account (A43) delete what they
  made. Starting The Commons again deletes it too, because it goes with the world's row.

## Limits

| What | Limit |
| --- | --- |
| Items in a yard | 40 |
| Pets on a tile | 4 |
| Rules on a pet | 6 |
| Changes a person can make | 60 a minute |
| Outsides waiting for each kind of place | 4 |
| Most one stored outside can come to | about 7 kilobytes, and under 8 |
| How often a page reads the outsides | every 30 seconds |

## Storage

Append to `SCHEMA` in `src/db.rs`. Existing tables don't change. An older binary doesn't read this
table, so going back is safe. It deletes a place without touching the row, and puts one up on a tile
without moving a waiting one onto it, so a row can be left for a tile with nobody's place on it, or
with another person's. A stale row shows nothing (E31), and a row left for the very place that stands
there shows again as it did.

```sql
CREATE TABLE IF NOT EXISTS place_outsides (
    world_id    TEXT NOT NULL REFERENCES worlds(id) ON DELETE CASCADE,
    player_id   INTEGER NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    outside     TEXT NOT NULL,                  -- the document, already checked (E2)
    stamp       INTEGER NOT NULL,               -- goes up with every change, for E26
    changed_at  INTEGER NOT NULL,
    PRIMARY KEY (world_id, player_id, kind, x, y)
);
```

## Routes

| Request | Body | Reply | Who |
| --- | --- | --- | --- |
| `GET /api/outside` | | `Catalog` | Someone signed in, as a person (E9) |
| `GET /api/places/{x}/{y}/outside` | | `{kind, outside, yours}` | Someone signed in, as a person |
| `PUT /api/places/{x}/{y}/outside` | `Outside` | `{outside}` | The place's owner (E6) |
| `DELETE /api/places/{x}/{y}/outside` | | `{ok}` | The place's owner or the keeper (E7) |
| `GET /api/worlds/{id}/outsides` | | `{outsides: [[x, y, outside]]}` | Anyone, limited like a view (E27) |

```text
Outside  { parts?: {part: Part}, body?: Body, items?: [YardItem], pets?: [Pet] }
Part     { style?, color?, number?, on?: [word] }
Body     { x, y, w, d, floors }
YardItem { kind, x, y, facing?, color? }
Pet      { species, name, coat?, collar?, rules?: [Rule] }
Rule     { when, do }
```

## Pages

The map's menu on your own place has **Build outside**. It opens the outside view over the map: the one
tile, large, with the sky behind it and a button to see it at night. The Build panel beside it is the
room's catalog, the same tiles in the same kind of list, with a colour row under anything that takes a
colour. Picking something from the yard holds it the way a room does: move over the tile, click to put it
down, and the dock under the picture has Turn, Put it here and Cancel, with Undo beside Done. Pets and
their automations are in the same list.

`web/outside.js` is the builder. `web/outside-art.js` puts a tile's layers in order, `web/outside-building.js`
and `web/outside-yard.js` and `web/outside-decor.js` draw them, `web/outside-spec.js` reads the document,
and `web/pets.js` draws pets and runs their rules.

## What this can't do

- **Shapes and colours can be rude.** Anyone can draw with a yard. The keeper can clear an outside or
  remove a player, but someone has to look.
- **A stamp can repeat if the clock goes back.** E26's stamp is the later of the clock and one more than
  the highest stored, and never goes back while the server runs. If it is restarted with its clock set
  back past a stamp that was the highest and has since been cleared, a page could miss that one change
  until the next.
- **`owner_watching` goes by the name on the map.** Names aren't unique, so someone who shares a name with
  a place's owner has its pet treat them as its owner on their own page. It changes nothing else.
- **Two people see different pets.** Rules run on each page, so a dog that's asleep on one screen can be
  awake on another. That's fine for a pet and would be wrong for anything that matters, which is why they
  do nothing else.

## Not built yet

- Windows and doors put wherever you like on a wall. For now a style and a count go on each wall.
- A pet that reaches into a room, tells its owner something or wakes a bot. That touches W1 and W59, so it
  needs its own design and its own rules.
- Bots decorating for their owner, the way a bot might ask to put a place up. Bots ask and people approve,
  so this needs a way to ask.
- Unlocking things with friends, as a room does. Everything here is free for now, and I19 to I21 say how it
  could work if that changes.
- Sound. Nothing on the site makes any.
