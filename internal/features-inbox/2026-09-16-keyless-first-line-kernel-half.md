---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Keyless first install: the kernel's half, and what the desktop can rely on

From the features agent, for the layout worker. QA: on a fresh place with
no model key, the first line the user typed was never delivered. The kernel
either ran a kickoff turn that only failed, or declined a `kickoff` in
silence; the desktop waited on the kickoff and queued the words behind it.

Kernel fix: [#312](https://github.com/unarbos/arbos/pull/312), e2e
`keyless_first_line_e2e`. Rule it enforces: nothing typed by the user is
silently held forever.

## What the kernel now does

**`kickoff` is always answered**, with one of three things:

| Kernel state | Frames back |
|---|---|
| Keyed, root has no turn on record | `turn {agent, state: running}` — the kickoff turn runs |
| **Keyless** | `error {agent, detail: "kickoff not started: No API key for … Your first message is kept and runs once a key is in place."}` then `turn {agent, state: idle}`. No kickoff turn is spent, nothing on the transcript. |
| Root already has a turn on record | `turn {agent, state: idle}` |
| A turn is running | nothing extra; that turn's own `idle` follows |

**A `user` line on a keyless kernel is kept, not burned.** The inbox file
stays (your pending row under the composer: `plan` frame, node with
`inbox: true`, `origin: "user"`, `goal: <the text>`). Once per agent the
kernel says so: a failed `notice` on the transcript naming the held text,
and an `error` frame with the same words. No `user` event and no
`turn_complete` until a key lands. `configure` with a key kicks the scan
and the held lines run in order — the `user` events land then.

Before this, each keyless line spent a failed turn ("root: turn failed"
notify, failed notice, `turn_complete`) and the user had to retype.

## What the desktop can rely on

- After sending `kickoff`, wait for **either** `turn running` **or** `turn
  idle` for root. A `turn idle` for root with no turn open means "the
  kickoff ended without running": release anything queued behind it.
  The 90 s `KICKOFF_QUIET` give-up can stay as a backstop, but it should
  never be what releases the words.
- A keyless `error` whose detail starts `kickoff not started:` is the
  setup card's cue, not a red line in the chat.
- A `user` you sent that comes back as an `error` naming the text plus a
  pending `plan` row was **kept**, not lost: draw the row, not a failure.
- `provider.key` still gates whether to send `kickoff` at all; that gate is
  right and can stay. What changed is that the kernel is safe if a client
  sends it anyway (the phone, a pre-#312 desktop).

## Not touched

The sibling finding — a line landing twice after a kernel respawn — is
the desktop's; nothing in #312 changes replay or the `user` record.
