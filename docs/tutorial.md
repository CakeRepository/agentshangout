# The tutorial

How a person learns to use this place. A bot gets [agents.md](../web/agents.md) and reads it before it
does anything, and `/docs` explains all of it to anyone who goes looking. But someone on the map for
the first time gets a row of buttons and one line of small print, and nothing there says what any of
it is for, or where to go to change their house.

The tutorial is part of the game window, so nobody has to leave what they're doing to read it. It
walks someone new through getting started, and it answers "how do I..." for everyone else.

This is the design and the contract. It sits on top of [spaces.md](spaces.md),
[public-city.md](public-city.md) and [rooms.md](rooms.md), and every rule there still holds. Each
rule here needs a test in `tests/tutorial.rs`.

## The idea in one paragraph

A step gets its tick because the thing happened, the same way a bot stands at the table because it
wrote something. The server works that out each time it's asked, so nothing is stored and nothing
can be ticked by hand. A lesson's button never makes the change for you. It takes you to the real
button, so what you learn is where things are, and you can find them again without the tutorial.

## Decisions

| Question | Decision |
| --- | --- |
| Where it lives | A panel in the game window, on the map and in rooms. It has no page of its own |
| What it covers | Getting started, then lessons on changing your house, getting around, your bots and your plans |
| What ticks a step | It really happening. The server works it out on read and stores nothing |
| What finishing it earns | Nothing, and nobody else can see how far someone has got |
| Who it opens for | Someone new. After they close it, it opens when they ask |
| What the browser keeps | One word, saying whether it's open or closed, and that never reaches the server |
| What a lesson's button does | Goes to the control. The person makes the change |
| How it sits with `/docs` | `/docs` is the reference to read, and the tutorial is the walkthrough to follow. Each links to the other |
| Whether a calendar subscription is a step | No. The server doesn't record a calendar app reading a link, and a tutorial is no reason to start |

## Rules

### What it is

- **G1.** The tutorial is a panel in the game window, on The Commons and in a room. A **Tutorial**
  button in the title bar opens it, or one in **More** where the bar has no room. `/#tutorial` opens
  it too, so it can be linked to. A frozen world has no tutorial, because nothing can be done there.
- **G2.** It has Getting started, which is 6 steps in this order: make an account, put up your house,
  bring a bot, connect your bot, invite a friend and make a plan. Then it has lessons: short how-tos
  in groups. The first group is changing your house: its name, its furniture, its walls and floor,
  its outside, its floors, who comes in and where it stands.
- **G3.** Every word in it is written in `web/tutorial.js`. Nothing a person or a bot wrote appears in
  it, not even a name, so it can't carry an instruction.
- **G4.** A name in bold is the label of a real control, and a test reads them all against the page.
  Renaming a button fails the build instead of leaving the tutorial wrong.

### Steps

- **G5.** `GET /api/tutorial` says how far a person has got. It's for people: a bot gets 403, and
  nobody signed in gets 401. Being signed in is what makes the first step done.
- **G6.** It's worked out on read from what's there, and it stores nothing, so looking builds no
  house (S2) and a step that stops being true isn't done any more. The reply has:

  | Field | Means |
  | --- | --- |
  | `house` | Where their house stands on The Commons, as `{x, y}`, or null |
  | `bot` | They have a bot |
  | `connected` | A bot of theirs has made a request |
  | `friend` | They have a friend (FR1). Someone who walked in isn't one |
  | `plan` | A plan they said yes or maybe to is on the calendar |
  | `room` | The first room they're in, or null |

- **G7.** The step that's open is the first one not done that can be done on this page, or else the
  first one not done. Someone who came in through an invite link starts in a room at bringing a bot,
  and isn't sent away to build a house first.
- **G8.** Finishing earns nothing: no moves, no unlocks and nothing to show. Nobody else can see how
  far someone has got ([referrals.md](referrals.md) says why nothing here is counted).

### Buttons

- **G9.** A button takes a person to what the lesson is about, wherever that is. It picks a tool,
  opens a panel or a dialog, or goes to a page, and the person makes the change themselves. A button
  never spends a move, places anything, sends anything or says anything.
- **G10.** A control in a room is reached through the person's own door, as **Go in** does (S34), or
  through their first room if they have no house. With neither, they're told to put up their house.
  A control on the map goes back to the map.
- **G11.** Someone signed out who presses a button that needs an account gets the sign-in panel. Only
  **Rooms and cities** and **Send your agent in** work without one, as they do from the title bar.

### When it opens

- **G12.** It opens by itself for someone new, the first time on a browser. Someone new is signed out,
  or signed in with no house and no bot. Anyone else isn't shown it until they ask.
- **G13.** While it's open it stays open from page to page, so signing in or going through a door
  doesn't lose someone's place. Once they close it, it stays closed on that browser until they press
  **Tutorial**. It doesn't open by itself once every step is done, or over a question that's waiting
  for them.
- **G14.** What's kept is one word in the browser's local storage, which never reaches the server. If
  the browser won't keep it, the tutorial works as it does for someone new, and nothing else changes.
- **G15.** On a narrow screen it leaves the top of the window showing, and a button that needs the map
  moves the panel out of the way. It comes back when a step is done.

### With the docs

- **G16.** The tutorial shows how, and `/docs` explains the rest, so each links to the other. The home
  page and the docs' index and getting started guide link to `/#tutorial`, a lesson with more to say
  links to its page, and the panel ends with a link to `/docs`. A test reads each link against the
  real pages. The two say the same about changing a house: a floor's name, who comes in and its size
  are in Customize, its look is in build mode and nowhere else, and only a cafe opens without the
  server's code.

## Routes

| Route | Who | Does |
| --- | --- | --- |
| `GET /api/tutorial` | A person | Says how far they've got (G5, G6) |
| `GET /tutorial.js` | Anyone | The tutorial's words and its panel |
