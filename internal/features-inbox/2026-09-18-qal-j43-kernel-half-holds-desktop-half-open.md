---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j43: the kernel's half holds — a chat minted mid-kickoff takes its first line. The loss is on the desktop's side of the socket.

**For:** QA, and the desktop owner (this is theirs to finish).
**From:** the features agent (kernel), 2026-09-18 16:20 UTC. Pin: [#669](https://github.com/unarbos/arbos/pull/669).

## What I drove

The bug's shape, from the kernel's side of the wire: root's kickoff turn running (a 6 s step), `arbos_core::create_chat` on disk from a second process — exactly what the desktop's `kernel::mint_chat` calls — a second attach, and a plain `user` frame for the new chat. Within a moment the line is in the chat's inbox, and the chat's turn runs to `turn_complete` during or right after root's kickoff. Three of three. `chat_during_kickoff_e2e` keeps it so.

So: no inbox file and an empty transcript means the frame **never reached the kernel for that agent**. The desktop is where the line stops.

## Where to look, in order (`desktop/src`)

1. **`agent/acp.rs` — the writer task.** `send_frame` pushes onto an mpsc and returns `Ok`; the task that writes to the TCP stream does `if writer.write_all(..).is_err() { break }` and says nothing. Once it has broken, every later `send_frame` still returns `Ok`, `prompt()` sees `sent == true`, and the composer clears — a positive acknowledgement for a line that went nowhere. If the reader half is still up, `live()` reads live too. Whether the new chat's socket loses its writer during kickoff is the first question; a log line on that `break` would answer it in one run.
2. **`agent/acp.rs:381` — `mint_chat`'s fallback.** `spawn_blocking(mint_chat).await.ok().and_then(Result::ok).unwrap_or_else(|| ROOT_ID)`: if `create_chat` fails (it calls `bootstrap(place)` first, on a place whose kernel is mid-kickoff and writing), the session silently becomes **root's**, and the typed line goes to root as a follow-up behind the kickoff — which would put it in `agents/root/inbox`, not the chat's. Your rollout can tell: look in root's inbox and transcript for the typed marker.
3. **`model/session.rs` — `send()` → `prompt()`**: `pending_wire`/`held_cards` paths land or skip the card, but none of them drop the line without queueing it; I do not think the fault is here, but it is the third place the line passes.

Either 1 or 2 fits "the app behaves as if everything worked". What must not survive, as you said: a cleared composer for an undelivered line — `prompt()` should clear only on a write the socket confirmed, or the writer's failure must close the connection so `live()` is honest.

## Not done here

No desktop change from me — the coordinator has the desktop slices with other workers. The kernel needs nothing for this one; the pin makes sure that stays true.
