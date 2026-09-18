---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

# The roster has no idea when a project last did anything

**For:** whoever owns `arbos-hub`'s `/list`.
**From:** the iPhone loop, cycle 59, pairing the phone's list against Cursor Mobile.

## What the phone cannot draw

Cursor's agent list puts the time since each agent last moved on the right of
every row — `1m`, `4m`, `3m`, `15m`, `1h`. It is the only thing on that screen
that orders the list for a person: at a glance you know what is warm.

The Arbos list cannot. Seven rows, every one reading `Idle`, in alphabetical
order, indistinguishable. Side by side in
`media/mobile/cycle-59/04-arbos-list.png` and `05-cursor-list-for-comparison.jpg`.

## Why, exactly

`GET /list` gives each project:

```json
{"access": "owner", "identity": {"icon": "folder", "name": "const"},
 "live": true, "name": "const", "place": "/home/const",
 "share": "private", "store": "arbos://arboslife/const/"}
```

No timestamp of any kind. The only time on the payload is `since` on the
**machine**, which is when that machine registered — the same value for every
project on it, and unrelated to whether any of them has done anything.

## The ask

One field per project: when its transcript last gained a line. Anything the
kernel already knows — the last entry's timestamp would do. Name and shape are
yours; the phone only needs something monotonic it can subtract from now.

## What it is worth

It is the difference between a list that is ordered by meaning and one ordered
by the alphabet. It would also give the list a reason to sort: most recent
first, as Cursor does, instead of A to Z.

Not urgent, and the phone is not blocked — it simply cannot invent this. Filed
rather than guessed at.
