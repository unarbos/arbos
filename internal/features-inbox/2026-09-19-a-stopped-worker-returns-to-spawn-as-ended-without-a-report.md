---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# A worker stopped by the person returns to `spawn` as "ended without a report", and the model says it finished

Found by the desktop symmetry loop, cycle 69, long-form d39 (`/tmp/d39-proj`,
`media/cursor-reference/cycle-69/d39/`), kernel
`arbos-kernel 0.2.0 e504bf2f48ad protocol 1`, model `google/gemini-2.5-flash`.

## What happened

The root spawned a worker with `wait=true` for `sleep 90; echo done`. The
person pressed **Stop All** on the window's Working card while the worker's
`bash` ran. The worker's transcript, verbatim in order:

```
tool bash        bash interrupted; job j1 killed
interrupted stop
turn_complete
```

The root's `spawn` call then returned:

```
tool spawn tool_spawn_44KEXWpVE4D   sleepy-worker reports: (the child's turn ended without a report)
```

and the model's next line to the person was **"The worker finished."** —
the opposite of what happened. (Its second `spawn` call in the same turn
went out with `{}` and was refused; that is the empty-brief case filed
earlier today, #764.)

The window's worker line under the root's answer read *Done · Sleepy
worker*: the desktop reads the worker's state from its records, and
`interrupted` followed by `turn_complete` is "done" to it today (desktop
F-238, to be drawn as *stopped by you* on the desktop side).

## What would hold

- `spawn`'s return for a child whose turn ended on `interrupted` should say
  so — *sleepy-worker was stopped by the user before it reported* — the way
  the child's own `Interrupted` record already does; "ended without a
  report" reads as the child's fault and lets the model call it finished.
- The same words in the wake the parent gets when `wait=false`.
