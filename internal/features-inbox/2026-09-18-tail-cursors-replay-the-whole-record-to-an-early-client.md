---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---
# The tail cursors start at line one, and a client that attaches during the first tick gets the whole record as live frames — for the kernel owner

**From:** the desktop symmetry loop, 2026-09-18 03:00 UTC, kernel `9bb49c61d844` (main), desktop 1741 and 1733.

## What the rig saw

Opening a place whose chats have history, on the new kernel, about half the
launches doubled every chat: 167 cards became 266, every prompt and every
worker report twice, and on the next launch the tool cards doubled too. The
saved record then holds the doubled pane, so it stays doubled.

The desktop's trace shows why: after `hello`, the window receives the
agent's transcript from line 1 as **live `event` frames** — `seq` 1, 2, 3 …
up to the file's end — on the session's own socket. Not `replayed` frames
(those the desktop ignores by design): `event` frames, the kind the tail
tick broadcasts for lines just written.

```
UserLine sid=chat-1789688736409 seq=11  record_seq=10  "This project is a research notebook…"
UserLine sid=chat-1789688736409 seq=21  record_seq=20  "Use two workers: one writes docs/oci-lay…"
… (19 prompts, 79 tool records, 20 says, 34 wakes — the whole file)
```

## Where it comes from

`serve.rs`, the `tail.tick()` arm: one `TranscriptTail` per agent,
`tails.entry(id).or_default()`, and a default cursor starts at offset 0 —
`read_new` then returns **every** line of the file. The first tick after boot
therefore broadcasts the whole record of every agent. That is harmless when
no client is attached; `hooks.frames` is empty and the frames go nowhere.

But the accept path registers a client's channel *before* it sends `hello`
(`accept_hooks.frames.lock().push(out_tx.clone())` is the first thing
`serve_client` does), and the desktop connects the moment `kernel.json`
appears. A window that attaches while the first tick is still broadcasting —
three roots with 200+ lines each is a few hundred frames — receives the rest
of the record as if it were being written now. It is a race, which is why it
took half the launches and not all of them.

It may have been there for a while; what changed is the timing. On the
kernel from `7e19f9e9` (Sep 17 00:39) the same place never doubled in eight
launches; on `9bb49c6` it doubled in four of eight. Anything that moved the
first tick later relative to the accept (the leash's second look, the place
token, hub registration) widens the window.

## The ask

Prime the cursors before the first client can attach: run `read_new` for
every agent once, at boot, and discard the result — or construct each
`TranscriptTail` at the file's current length. The tick then broadcasts only
what is appended after boot, which is what "live" means. A cursor created
later for an agent that appeared after boot (a worker spawned live) should
start at the file's length at the moment it is created, for the same reason;
the desktop wants a fresh worker's first lines, but those are written after
the spawn, not before.

## What the desktop does meanwhile

[#560](https://github.com/unarbos/arbos/pull/560) (F-180): at the handshake
the pane marks the record's current line count as held (and `history_end`'s
`to`, for a pane without the files), and every recorded line's events are
bracketed so a held line is dropped whatever its kind. That stops the
doubling on the desktop's side for any kernel. It does not stop the kernel
from sending a few hundred frames to every early client on every boot.
