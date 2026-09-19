---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# `spawn` called with `{}` ends the turn with the model asking the person for a `task` or `brief`

Found by the desktop symmetry loop, cycle 58, prompt p6 (`/tmp/p6-proj`,
`media/cursor-reference/cycle-58/p6/`), kernel
`arbos-kernel 0.2.0 f02aadae63df protocol 1`, model `google/gemini-2.5-flash`.
Two of three runs of the same prompt today ended this way; the third spawned
and worked.

## What happened

The person typed, in a fresh place after the kickoff turn:

> Spawn one worker with this brief: 'Run this exact bash command with the bash
> tool and report the last line it printed: for i in $(seq 1 34); do echo tick
> $i of 34; sleep 1; done'. Await it and tell me the result.

The controller's first step called `spawn` with no arguments at all. From
`.arbos/agents/root/transcript.jsonl`, verbatim:

```
{"name": "spawn", "args": {}, "error": "spawn: give `task` (with the template fields) or a raw `brief`", "step": 1}
```

The tool's refusal (`tools.rs:581`, the `(None, None)` arm) went back to the
model as the step's result, and the model's next step was its reply to the
person:

> I need a `task` or `brief` argument for the `spawn` tool. Could you please
> provide…

The pane then shows a *Worked 2s* fold with the failed `spawn` card inside and,
under it, a reply that names a tool argument to the person — who had already
written the brief, in quotes, in the line above. Cursor never surfaces a
tool's parameter names to the user; a bad tool call is retried by the model.

The first run of p6 did the same with a plainer line (*Run this exact bash
command and then tell me the last line it printed: …*): `spawn` with `{}`,
then *I need more information to spawn a worker. Please provide a `task` or
`brief` argument.*

## What would hold

- The refusal text is written for the model, not the person, and should say
  what the model can do: *put the person's request in `brief`* — or the
  kernel fills `brief` from the turn's user line itself when both are empty
  (the line is one `turn_user_text` away, and the `show` check already reads
  it).
- Or a nudge: a step whose only tool call was refused for missing arguments
  gets one more step with the refusal before the turn may end in prose.
- Either way the person's pane should not read a tool's argument names.

Desktop side, nothing to change: the failed card and the reply are drawn as
the kernel sent them.
