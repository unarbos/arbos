---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# The parent's "waiting on <worker>" status line — what to draw

For the layout worker, from the features agent.
[#366](https://github.com/unarbos/arbos/pull/366).

## Why

Jacob's Mac: a coordinator sat silent for 2m 26s while its worker ran
`python3 bubble_sort.py`; he typed "run it" four times. Cursor keeps the
person informed while a delegate works. The kernel now does it on the
**status line**, not the transcript, so the chat stays quiet.

## What the kernel sends

A `status` frame for the *parent* with a new `source`:

```json
{"type":"status","agent":"root","step":"waiting on Slow builder — Running sleep 5; echo built","since":"2026-09-17T00:41:10Z","source":"waiting"}
```

- `step` is `waiting on <name> — <the worker's own step>`; the name is the
  worker's `name` as the user gave it (`Slow builder`), the step is the
  worker's live line as you already draw it on the worker's row.
- Several live workers: `waiting on 2 workers — <name>: <step>` (the one
  whose step changed last).
- A worker with no step yet: `waiting on <name> — starting`.
- `since` is the **worker's** clock, carried over, so your timer reads the
  worker's time on the step (not the time since the parent last copied
  it).
- It is refreshed as the worker's step changes, and cleared — an empty
  `step` frame for the parent, the file removed — the moment no worker
  is live. No stale "waiting on" is left behind; if you see one, it is a
  bug, tell me.
- Both shapes: the parent idle after a spawn-first turn, and the parent
  blocked in `spawn wait=true`.
- The parent's own `status` tool line (source `agent`/`title`) is never
  overwritten by this.

## Suggested drawing

Where the parent's live line already goes (the roster row, the chat
header). The `waiting on <name> — ` part dim, the worker's step in the
live-line style, so it reads as *someone else's step, watched*. Nothing
new to fetch: a client attaching mid-wait finds the line in the parent's
`status.toml` with the snapshot, as with any status.

## Not changed

The worker's own row still shows its own step. The derived step text
(`Running <command>`, 40 chars) is as before; if you would rather the
line used the model's `description` ("Building the thing") where one is
given, say — one line in `status::derived`.
