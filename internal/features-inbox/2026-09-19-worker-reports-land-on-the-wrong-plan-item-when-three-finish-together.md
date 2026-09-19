---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Worker reports land on the wrong plan item when three finish together

Found by the desktop symmetry loop, cycle 53 (`three_workers.py`, stills under
`media/cursor-reference/cycle-53/three/`), kernel
`arbos-kernel 0.2.0 f02aadae63df protocol 1` on `main` `1a0b17aa`.

## What happened

The root was asked to spawn three workers with `wait=false`, await all three
and report together. The workers were named *Worker A* (a markdown table),
*Worker B* (`ls /definitely-not-here`, report the error) and *Worker C*
(reply *three*). All three finished within seconds of one another.

`.arbos/notes.md` afterwards, verbatim:

```
## Running Workers
- [x] [Worker C](agents/worker-c) — replied with 'three'
- [x] [Worker B](archive/agents/worker-a) — worker finished: The markdown table of Python files and their line counts is available at `.arbos/docs/python_file_summary.md`
- [x] [Worker A](agents/worker-b) — worker finished: The command `ls /definitely-not-here` resulted in the error: `ls: cannot access '/definitely-not-here': No such file or directory`
```

*Worker B*'s item points at `archive/agents/worker-a` and carries A's report;
*Worker A*'s item points at `agents/worker-b` and carries B's. The labels are
the model's; the `— worker finished: …` readouts and the archive-path rewrite
are the kernel's, and they went to the items by position, not by the worker
each item names.

The root's own transcript had the reports right (*Worker B done — The command
`ls /definitely-not-here` …*, *Worker A done — The markdown table …*): only
the page is crossed.

## What the desktop shows

The panel's Project section draws the page as written — *Worker B · worker
finished: The markdown table …* — so a person reading the page is told the
wrong worker did the wrong thing. The pane has no way to know; the fix is
where the readout is written: match the finishing worker's item by its link
target (`agents/<id>` or its archive path), not by order of finishing.
