---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# A parked `ask` outlives the `say` that woke the agent, and is replayed as pending at every attach

Found by the desktop symmetry loop, cycle 58, prompt p6 (`/tmp/p6-proj`,
agent `run-bash-command`), kernel `arbos-kernel 0.2.0 f02aadae63df protocol 1`
(the same on `e504bf2f`'s build).

## What happened

The worker asked (`ask` tool, call `tool_ask_IFsrcUCyopfahoOHz0QU`), the
kernel parked the turn and wrote the question under `waiting/`:

```
agents/run-bash-command/waiting/ask-tool_ask_IFsrcUCyopfahoOHz0QU.toml
kind = "ask"
question = "What bash command should I run?"
asked = "2026-09-19T11:17:08Z"
```

The parent then answered with `say`. The transcript, in order:

```
ask   What bash command should I run?
tool  ask
notice Waiting for your answer
turn_complete
say   root      ← "The bash command to run is: for i in …"
wake
tool  bash      ← the command ran; the worker finished its task
…
turn_complete
```

The agent took the `say` as its next message, ran the command, and finished.
The `waiting/ask-…toml` file is still there — and `serve.rs`'s attach path
(*Questions still parked on the user: offered again, with their ids*) sends
`Frame::Ask` for it at **every** attach, so a window that opens this chat an
hour later gets the card *What bash command should I run? · Skip · Continue*
over a finished transcript, and its composer reads *Add more optional
details*.

## What would hold

- A wake that starts a turn on an agent with a parked ask closes the ask
  (removes or marks the `waiting/` file) — the agent has been answered by
  the `say`, or has moved on without one; either way nobody is waiting.
- Or: the attach replay skips a `waiting/` ask whose `asked` time is older
  than the agent's newest `turn_complete`.

Desktop side, in [#756](https://github.com/unarbos/arbos/pull/756): the pane
drops a question when a line from another agent, a person's line or a wake
follows the ask's own record — so the replayed card no longer shows. The
kernel's file is the thing to fix; the desktop only stops drawing it.
