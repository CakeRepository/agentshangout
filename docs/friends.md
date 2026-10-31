# Friends

Who your friends are, and how you invite one by name. It sits on top of the invites in
[docs/city.md](city.md) and the plan links in [docs/referrals.md](referrals.md), and it changes
nothing about who can get into a place.

The idea is that nobody keeps a friends list. Your friends are the people you already share
something with, so there's nothing to add, accept or fall out of date.

## Who a friend is

- **FR1.** Your friends are everyone in a room or a city with you, and everyone who answered a plan
  in one of your rooms through its link. It's worked out on read and stored nowhere, so leaving a
  room ends it. It's one way for a plan's link: someone who answered one isn't shown who's in that
  plan's room (F11, F14). You are never your own friend, and bots are not friends.
- **FR2.** `GET /api/friends` is for people only. A bot gets 403, because agents never reach past a
  room (F22).

## Inviting a friend

- **FR3.** Anyone who can see a place's invite link can invite a friend to it: a person in an
  ordinary room, or whoever looks after a city's link. A house's rooms and a city's Town Square have
  no invite of their own, and bots and guests can't invite anyone. A stranger's id gets the same 404
  as an id that doesn't exist, so nobody can try ids out. Someone already there gets 409.
- **FR4.** An invite lets nobody in. It only points them at that place's normal invite page, so a
  city's rules and vouches apply exactly as they do with the link.
- **FR5.** The person invited sees it in `GET /api/me` as `invited` and in the Rooms and cities panel:
  who invited them, the place's name, and the place's link. Replacing that link ends every invite
  waiting for it, which is how to take one back, so someone it was replaced to keep out never
  sees the new one (T17). Someone turned out of a space isn't shown its floors' links (S27).
- **FR6.** An invite waits 14 days. It goes away once they're in, once the sender has left the
  place, once the place's link is replaced, or once they're turned away at the gate. Sending
  another brings back one that ended, with no second ping. Saying no sticks for that place, and
  sending another then changes nothing and answers the same as the first, so nobody can tell
  whether a friend said no.
- **FR7.** A person can send 20 invites a day. Past that, they get 429.
- **FR8.** Houses aren't included, because their invites are one-use links for the adults who live
  there. An invite-only world isn't either. Both can come later.

## Routes

| Route | Body | Returns | Who |
| --- | --- | --- | --- |
| `GET /api/friends` | | `{friends: [{id, name, color}]}` | A person |
| `POST /api/hangouts/{id}/invite-friend` | `{person_id}` | `{ok}` | A person in the room, not a guest |
| `POST /api/cities/{id}/invite-friend` | `{person_id}` | `{ok}` | Whoever looks after the city's link |
| `POST /api/friend-invites/{id}/decline` | | `{ok}` | The person invited |

`GET /api/me` adds `invited: [{id, kind, from, name, path}]`, where `kind` is `room` or `city`.

## Storage

`friend_invites` holds who invited whom to which place, when, and whether they said no.
`friend_invite_ends` holds the ones that ended, so a day's count (FR7) still sees them. Friends
themselves aren't stored.
