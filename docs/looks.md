# Characters

Everyone has a figure, and it's the same someone wherever it is. A figure's look used to come from its
name, so nobody had a say in it. Now a person chooses how they look and how each of their bots looks,
in the Character panel on the room page or The Commons, the way they furnish a room in build mode.

This is the design and the contract. It sits on top of [rooms.md](rooms.md), [spaces.md](spaces.md) and
[public-city.md](public-city.md), and every rule there still holds. Each rule here needs a test in
`tests/looks.rs`.

## The idea in one paragraph

A look is a few words from a fixed wardrobe, one for each part of a figure, so nothing typed reaches
the drawing. A part nobody chose stays what the name gives it, so a figure nobody touched looks the way
it always did in a room. The server says what each figure wears, so a room, a city's map and The Commons
draw the same words in their own art: they can look different, but they can't disagree. A look is for
looking at: it can't say whose a bot is, and it can't turn a person into a bot.

## Decisions

| Question | Decision |
| --- | --- |
| What's a look | A word for each part, from a wardrobe in the code |
| Who changes one | A person, for themselves and their own bots. A bot can't change any look |
| What it costs | Nothing. Nothing is locked, and nothing unlocks |
| What's stored | Only what someone chose |
| Where it shows | Everywhere its wearer is drawn: rooms, a city's map and The Commons |
| Who has none | Guests, visitors and anyone not signed in, who are drawn from their generated names |

## Rules

### The wardrobe

- **C1.** A look is a word for each part of a figure, and every word comes from the wardrobe in the
  code. A person's parts are `skin`, `hair`, `style`, `face`, `shirt`, `trousers` and `hat`. A bot's
  are `shell`, `head`, `eyes`, `chest` and `hat`. Anything else is refused with 400, so nothing typed
  is ever part of a look.
- **C2.** A person takes a person's parts and a bot takes a bot's, so a look can't draw a person as a
  robot or a bot as a person (W53).
- **C3.** Nothing is locked. All of the wardrobe is open to everyone from the start, nothing unlocks a
  word, and a look does nothing but change how a figure is drawn. The exception in I20 stays the only
  one.

### What's chosen and what isn't

- **C4.** Only what someone chose is stored. A part with nothing chosen is worked out from the name,
  and a shirt or a shell comes from the owner's colour. So someone who never opens the panel looks in
  a room as they always did.
- **C5.** A change names the parts it changes. A part left out stays as it is, and `null` takes a part
  back to C4. A change that's refused changes nothing, even if some of it was fine.
- **C16.** A figure wears one look everywhere. The server works out what it's wearing, chosen or not,
  and says so in words, and every page draws those words. No page works out a look for anyone with an
  account, so a room and a map can't disagree about someone. Shirt, skin, hair and hat are the same
  words in both, and only the art differs. A bot's map figure used to come from its own sums, so a bot
  nobody dressed now looks on a map as it does in a room.
- **C6.** A stored word that isn't in the wardrobe is left out on read, so dropping a word from the
  wardrobe or going back to an older release can't break a page.

### Who changes what

- **C7.** A person changes their own look and the looks of their own bots, as a person. A look belongs
  to whoever wears it and not to a floor, so someone who walked in can change theirs, which isn't so
  for an item (I2). Someone else's bot gets the same answer as one that doesn't exist, and a house bot
  follows R44.
- **C8.** A bot can't change any look, its own included. That's 403, and there's no tool for it.
- **C9.** A person can make 60 changes a minute, across themselves and their bots. Past that, 429.
  Each change refreshes every room the figure is in (C10), so this keeps one person from refreshing
  everyone else's pages without end.

### Who sees it

- **C10.** A change refreshes every room the figure is in, and a bot's refreshes the maps of its
  owner's cities and The Commons too, so people see it as it happens, as with items (I6).
- **C11.** A room's snapshot has `looks`: `people` for those with the floor open (S42) and `bots` for
  the room's bots, guests included, each by id. It holds what each is wearing (C16), and it's empty
  for a bot.
  `read_room` says nothing about looks, because they're for drawing and not something a bot reads.
- **C12.** A figure's name tag keeps its owner's colour whatever it's wearing. A look changes the
  drawing and nothing else, so no shell can pass a bot off as someone else's.
- **C13.** A city's view has `bot_looks`, by id, for the bots on its map. It's empty for a bot.
- **C14.** A look goes where its wearer goes, and that includes The Commons. A person's own player
  and a bot's player have what they're wearing as the seventh field of their `players` row. Someone
  signed in with The Commons open has it as a fourth field of their `people` row until they're on the
  street as their own player (W52), and a bot that L17 shows has it in `bots_out`. That's words and
  nothing else. A player's name stays generated, a place's `owners` row has no look, and no id or
  account name goes with it, but someone who knows a look can recognise it out there, and that's the
  point (W4). A guest, a player someone claimed, a visitor and anyone not signed in have no look.
- **C15.** `GET /api/looks` gives a person the wardrobe, with what they and the bots they can change
  have chosen and are wearing. It doesn't say what anyone else wears. A bot gets 403.

## Limits

| What | Limit |
| --- | --- |
| Changes to looks | 60 a minute for each person |

## Routes

| Request | Body | Reply | Who |
| --- | --- | --- | --- |
| `GET /api/looks` | | `{parts: {person, bot}, you, bots}` | People |
| `PUT /api/me/look` | `{part: word or null}` | `{look, worn}` | People |
| `PUT /api/agents/{id}/look` | `{part: word or null}` | `{look, worn}` | Its owner |

`look` is what was chosen, and `worn` is that with everything else worked out (C4, C16).

## Storage

Append to `SCHEMA` in `src/db.rs`. Existing tables don't change. A part is a row, so a new part needs
no new table, and an older binary just never looks at these.

```sql
CREATE TABLE IF NOT EXISTS person_looks (
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    part        TEXT NOT NULL,
    choice      TEXT NOT NULL,
    PRIMARY KEY (person_id, part)
);
CREATE TABLE IF NOT EXISTS bot_looks (
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    part        TEXT NOT NULL,
    choice      TEXT NOT NULL,
    PRIMARY KEY (agent_id, part)
);
```

## Not built yet

- A bot choosing its own look. It would need its own tool and its own rule first.
- A person on a city's map, which only draws bots.
- Drawing rooms in the map's art, so inside and outside look alike.
