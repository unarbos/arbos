---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# A fork's `say` resumes the original chat's worker, and the worker's report goes to the original

Found by the desktop symmetry loop, cycle 76, the rewind/fork item
(`/tmp/stop75b-proj`, `media/cursor-reference/cycle-76/rewind-fork/`), kernel
`arbos-kernel 0.2.0 e504bf2f48ad protocol 1`, model `google/gemini-2.5-flash`.

## What happened

The root spawned a worker with `wait=true` (`sleep 90; echo finished`); the
person pressed Stop while it ran. The chat was then forked from the desktop
(the kernel's `clone_session`), and on the fork the person pressed **Continue
Working** (*Continue where you stopped.*).

The fork's transcript (`agents/chat-1789858251108/transcript.jsonl`), in order:

```
wake       Continue where you stopped.
user       Continue where you stopped.
tool       say            (to=runthisbashcommandandwai)
nudge      Your reply was empty. Continue the task, or say what is blocking you …
assistant  The `runthisbashcommandandwai` worker has been messaged to continue. I'm waiting for its report.
turn_complete
```

The original root's transcript (`agents/root/transcript.jsonl`) after that:

```
wake       Report from runthisbashcommandandwai above — the last of your workers. If the user is owed an answer, give it …
assistant  The previous command was stopped by the user. Should I continue with the task or start over?
turn_complete
```

So: the fork's model read the copied transcript, found a worker id there,
and `say`-ed to it. The worker (a child of the *original*) ran and reported
to its parent — the original woke and answered a question nobody on that
chat had asked. The fork waits for a report that will never reach it; its
pane reads *I'm waiting for its report* and no worker line (it has no
children).

## What the desktop does with it

Nothing it can: the fork has no children of its own, so it draws no worker
line, and the `say` is a plain tool row. The pill row on the fork carries
*Continue Working* (the stopped turn was copied), which is right.

## What the kernel could do (one of)

- A clone that knows it is a clone: the copied transcript's worker ids are
  not the fork's — `say to=<a worker of another chat>` refused with the
  owner named (*runthisbashcommandandwai belongs to root; spawn your own*),
  or the say allowed and the worker's report routed to the sayer as well.
- The clone strips or rewrites the original's worker ids in the copied
  transcript, so the model does not learn them.
- At the least, the wake on the original says who woke the worker
  (*resumed from chat-1789858251108*), so the original's model does not
  read it as its own doing.

## Where it is

Kernel `clone_session` and the `say` tool's addressing (`tools.rs`), and the
done-message routing in `plan.rs` (`parent` of the worker).
