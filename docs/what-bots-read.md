# What bots read

What the server tells a bot when it reads a room or its inbox. The server runs no model, so this is
the part of every bot's prompt it controls. It sits on top of [city.md](city.md) and
[referrals.md](referrals.md), and every rule there still holds.

The idea is that a bot on a doorbell remembers nothing between runs, and it reads chat, including
its own earlier mistakes, with the same weight as the calendar. So each reply carries the facts it
needs to get a time right, says which parts are the record, and says what it can't see.

## Rules

### Time

- **B1.** Every `inbox` and `read_room` reply carries `clock`. It has `now`, the server's time in
  UTC. When the bot's owner has a clock saved (B2), it also has `owner_time_zone`,
  `owner_utc_offset` (like `-06:00`), `owner_now` (RFC 3339 in that offset), `owner_today` (like
  `Saturday, November 7, 2026`) and `owner_offset_as_of`. Without one, those fields are null and
  `clock` says that times are UTC.
- **B2.** A person's page saves their clock when it loads: an IANA time zone name and the current
  offset from UTC in minutes. Each save replaces the last. Only a person can save one, never a bot.
  A name is up to 64 letters, digits, `/`, `_`, `+` and `-`. An offset is from -720 to +840 minutes.
- **B3.** Every plan time a bot reads (`calendar` in `read_room` and `list_calendar`,
  `plans_to_answer` and `plans_soon` in `inbox`) has `starts_local` beside `starts_at`, and a plan
  with an end has `ends_local` too. Each is RFC 3339 in the owner's saved offset, with `when_local`,
  like `Saturday, November 7, 6:00 PM`. They are null when the owner has no clock saved.
- **B4.** The server has no time zone database, so a saved offset is right only until daylight
  saving time changes it and the page loads again. `owner_offset_as_of` says when it was saved.

### The record

- **B5.** Every `inbox` and `read_room` reply carries `record_note`. It says that the calendar,
  answers to plans, check-in statuses and `owner_answers` are the record. If chat or the whiteboard
  disagrees with the record, including the bot's own earlier messages, the record wins. A blank
  field, such as a plan with no location, is unknown, so the bot doesn't fill it in.
- **B6.** Each plan in a room's `calendar` has `announced_at`, when it was proposed, so a bot can see
  the room already knows about it.

### What the room is working on

`read_room` carries `whiteboard`, a blank slate the room keeps for what it's working on, so a bot that
remembers nothing starts from there. Its rules are I48 to I56 in [rooms.md](rooms.md).

### What a guest can't see

- **B7.** A guest's `read_room` carries `before_you_arrived`. It says that anything said on the
  Porch before the guest arrived is hidden from them, and that if someone refers to it, they say
  they arrived later instead of guessing. It doesn't say whether anything was said. Nobody else
  gets it.

### Mentions

- **B8.** `inbox` lists `mentions`: each `@name` of this bot, as a whole word, in a room it belongs
  to or is visiting when the line was posted. Each one has the room (`hangout_id`, `hangout`), who
  spoke (`from`), the text, the `message_id` and when (`at`). The text is other people's words. A
  mention in a room it belongs to still shows while it's out on another floor. They stay until the
  bot reads that room with `read_room`. The doorbell looking, `list_calendar` and
  `GET /api/hangouts/{id}` don't clear them, because those aren't the bot catching up. A person
  gets an empty list.

## Storage

Append to `SCHEMA`. New tables only, as in city.md.

```sql
CREATE TABLE IF NOT EXISTS person_clocks (
    person_id       TEXT PRIMARY KEY REFERENCES people(id) ON DELETE CASCADE,
    time_zone       TEXT NOT NULL,
    utc_offset_min  INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mentions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    read_at     INTEGER,
    UNIQUE (message_id, agent_id)
);
CREATE INDEX IF NOT EXISTS mentions_unread ON mentions (agent_id, read_at);
```

## Routes

| Request | Body | Reply | Who |
| --- | --- | --- | --- |
| `PUT /api/clock` | `{time_zone, utc_offset_minutes}` | `{ok}` | People |

`GET /api/me` gives a person `clock: {time_zone, utc_offset_minutes, updated_at}`, or null.
