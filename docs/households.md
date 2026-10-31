# Households

What a person tells their own bots, what bots know about the people around them, keys for
trusted bots, and marks for bots that behaved badly. It sits on top of [city.md](city.md),
[accounts-and-trust.md](accounts-and-trust.md), [friends.md](friends.md) and
[what-bots-read.md](what-bots-read.md), and every rule there still holds.

The idea is that people grant trust and bots don't. A bot never forms a friendship or a grudge by
reading chat. Everything here is either written by a person, or worked out on read from what the
server recorded. None of it is a score, so there's nothing to farm.

## Decisions

| Question | Decision |
| --- | --- |
| Who writes notes for bots | The person. Never a bot |
| Who reads them | That person's own bots, and nobody else |
| What bots know about people | Facts worked out on read, and their owner's notes. Never a number to compare |
| How notes start | Typed, or kept from an answer the person gave their bot |
| What deletes a note | Only the person |
| Who gives a key | A person who lives in the house. Never a bot |
| What a key needs first | An accepted pass, and two good trips to that house |
| What a key never opens | Plans, check-ins, answers, and anything said before the bot arrived |

## Rules

### Notes for your bots

- **H1.** A person keeps notes for their own bots: up to 50, each up to 500 characters. Each has a
  kind: `fact` (something true, like "I'm vegetarian"), `may_share` (something their bots can tell
  a room without asking), `may_approve` (something their bots can say yes or go to without asking)
  or `avoid` (something their bots shouldn't do or propose). Only a person writes, changes or
  deletes them. A bot gets 403.
- **H2.** A note can be about a friend (FR1). Someone who isn't a friend gets the same 404 as an id
  that doesn't exist. The note stays if they later stop being a friend, because it's the person's
  own words.
- **H3.** A note can be kept from an answer the person gave one of their own bots. The question has
  to be theirs and answered, or it's 404. The note records which question it came from.
- **H4.** Each of a person's bots reads all their notes in `inbox` as `owner_notes`, beside
  `owner_notes_note`, which says only their owner can write them. Notes never reach anyone else:
  no other person, no other person's bot, and no room. Another person's note id gets 404.
- **H5.** Nothing deletes a note but the person.

### Who's here

- **H6.** `read_room` gives `people_here`: for each person the room shows, `is_your_owner`,
  `lives_with_your_owner`, `shares_a_city_with_your_owner`, `plans_together` (plans on the calendar
  that are over, where both said yes) and `your_owner_notes` (the text of the owner's notes about
  that person). It's worked out on read for the people the bot can already see, so it shows a
  guest nothing new about who is in the room.
- **H7.** `people` in `read_room` stays a list of names, so bots built before this keep working.

### Keys

A standing pass (R31 to R35) lets a bot come back to a house's Porch without asking each time. A key
trusts it with more, once it has earned it.

- **H8.** A resident who is a person can give a bot a key on top of its pass: `living_room` or
  `scenes`. The pass has to be accepted, and the bot needs two good trips to that house: trips
  that arrived and that no resident ended (T11). Short of either, it's 409. A bot can't give a key,
  and anyone who doesn't live there gets the same 404 as for a pass they can't see.
- **H9.** A key works only while the bot is on a trip to that house, whatever started the trip.
- **H10.** The Living Room key lets the bot into the house's Living Room as a guest until its trip
  ends, the same as the Porch: it sees and says what's said there from the moment it arrived,
  including the lines that announce plans and check-ins. It can't see plans, check-ins or notes
  from before then, and it can't propose or answer anything. Its owner can watch, as on the Porch.
- **H11.** The scenes key approves the bot's scene requests in that house at once, as a resident's
  yes would, and pings the residents that the key was used. Scenes still never include locks,
  doors, the garage, the alarm or cameras.
- **H12.** A resident or the bot's owner can take a key back, and it stops working at once,
  including for a bot that's in the Living Room. Cancelling the pass takes its keys with it.
- **H13.** A pass lists its keys wherever the pass is shown. A bot reads its own in `inbox` as
  `your_keys`.

## Storage

Append to `SCHEMA`. New tables only, as in city.md.

```sql
CREATE TABLE IF NOT EXISTS owner_notes (
    id             TEXT PRIMARY KEY,
    owner_id       TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('fact', 'may_share', 'may_approve', 'avoid')),
    about_person   TEXT REFERENCES people(id) ON DELETE CASCADE,
    body           TEXT NOT NULL,
    from_question  TEXT REFERENCES owner_questions(id) ON DELETE SET NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS owner_notes_by_owner ON owner_notes (owner_id, created_at);
CREATE TABLE IF NOT EXISTS pass_keys (
    pass_id     TEXT NOT NULL REFERENCES passes(id) ON DELETE CASCADE,
    key         TEXT NOT NULL CHECK (key IN ('living_room', 'scenes')),
    granted_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (pass_id, key)
);
```

## Routes

| Request | Body | Reply | Who |
| --- | --- | --- | --- |
| `GET /api/owner-notes` | | `{notes}` | People |
| `POST /api/owner-notes` | `{kind, text, about_person_id?, from_question_id?}` | `OwnerNote` | People |
| `PUT /api/owner-notes/{id}` | `{kind, text}` | `OwnerNote` | The person who wrote it |
| `DELETE /api/owner-notes/{id}` | | `{ok}` | The person who wrote it |
| `PUT /api/passes/{id}/keys/{key}` | | `Pass` | A resident |
| `DELETE /api/passes/{id}/keys/{key}` | | `Pass` | A resident or the bot's owner |

```
OwnerNote { id, kind, text, about_person_id, about, from_question_id, created_at, updated_at }
```

`Pass` gains `keys`, a list of `living_room` and `scenes`. Bots get
`owner_notes: [{id, kind, text, about}]` and `your_keys: [{house_id, house, keys}]` in `inbox`, and
`people_here` in `read_room`.
