# qal-j43 — a chat opened during a place's kickoff turn silently eats what you type

- **status**: reproduced 4/4, deterministic; product side unfixed
- **found**: 2026-09-18 15:35, chasing `qal-j42`'s residual
- **app**: `1beec0a1fd98`
- **kernel**: `arbos-kernel 0.2.0 cecd48e1bd76 protocol 1`
- **split from**: `qal-j42` (which was the `new-subchat` control moving; this is a different defect
  found underneath it)

## What happens

Open a new place. Within its first ten seconds, press ⌘N and type a line.

The app behaves as if everything worked. The chat is minted, becomes the active tab, gets an id
(`chat-1789745785102`), and reports its connection **live**. `composer-field` accepts the keystrokes
and **clears on Enter**, which is the app's own signal that a line was accepted.

The kernel never hears it. The agent's `transcript.jsonl` is **never created**, no inbox file is
written, and no turn ever starts. The line is gone with no notice of any kind.

## Why the first ten seconds

A brand-new place serves a **kickoff turn** — "this place was just opened for the first time; this is
its kickoff turn" — which takes ~10 s. A chat minted while that turn is in flight is inert. Minted
after it completes, the same chat works.

Four runs, two per arm, one fresh place each, same app and kernel:

| arm | kickoff running when the chat was minted | the chat's transcript | typed line landed |
|---|---|---|---|
| during kickoff | yes | `[]` — never created | **no** |
| during kickoff (repeat) | yes | `[]` | **no** |
| after kickoff | no | `['wake', 'user']` | yes |
| after kickoff (repeat) | no | `['wake', 'user']` | yes |

The app's own log carries the mismatch from its side:

```
session 2 (chat-…): filed parent Some("root"), the kernel's record says None; the record wins
```

The app filed the chat under `root`; the kernel had no record to file it against, because the place
was mid-kickoff. "The record wins" — and the record does not exist, so the chat has nowhere to send.

## Why this matters beyond the harness

This is not a test-rig artifact. The window is real time on a real clock, and opening a place and
immediately starting a chat is ordinary behaviour — it is arguably the *most* likely thing a person
does in a place's first seconds.

The failure mode is the worst available one: **silent input loss with a positive acknowledgement.**
The composer clearing is how the app tells you it took your line. It clears here too. Nothing in the
UI distinguishes this from a working send; the turn simply never starts, which reads as slowness
rather than loss, so a person's natural response is to wait rather than retype.

That puts it in the same family as `qal-j33` (a click that did not land looked like a send) and
`#441` (a held place said nothing) — Arbos losing work quietly rather than refusing it loudly.

## Two shapes a fix could take

1. **Make minting wait.** ⌘N during kickoff queues until the place has a record to parent against.
   Nothing in the UI changes; the chat just starts a beat later.
2. **Make the composer hold.** The chat mints immediately but the composer keeps the line, and does
   not clear, until an agent exists to receive it. Slower to build, and the honest one: it never
   acknowledges a line it has not delivered.

Either is better than the current behaviour. What must not survive a fix is a **cleared composer for
an undelivered line.**

## How the loop found it, and what the loop now does

This hid behind `qal-j42` for four cycles. Every scenario that failed called `new_chat()` as its
first act, milliseconds after launch; every probe that worked happened to poll for ~12 s first. Both
orderings were stable, on opposite sides of the kickoff turn, so the bug looked like "works by hand,
fails in the harness" and sent me through seven wrong eliminations (ns-wrap, scratch `HOME`, the
payload's backticks, the predicate, the driver's field names, the agent's missing parent, and the
harness's `config.toml`). Each of those was a real difference between probe and harness. None was
the cause. The variable I had not controlled was **time**.

The lesson for the library: when a probe and a scenario disagree and every *substantive* difference
is cleared, suspect **when**, not **what**.

`desktop_scenarios.new_chat` now calls `wait_kickoff_done()` before minting. That is a
**scenario-side workaround for this bug**, marked as such in the helper, and it is why `mt-01`,
`mt-04` and `dg-01` now get past the door. A green desktop leg is therefore **not** evidence this is
fixed — only that the loop stopped tripping over it. The check that would catch a regression here
has to mint during kickoff on purpose.
