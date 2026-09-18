# qal-j41 — two spawn-brief scenarios assert on the arguments instead of the brief the worker got

- **status**: `mt-07` fixed and verified; `mt-26` open as a question about the kernel's brief, not the worker
- **found**: 2026-09-18 14:03, triaging cycle 9 half B
- **kernel**: `arbos-kernel 0.2.0 a8678ac16636 protocol 1` (the breaks), `cecd48e1bd76` (the re-runs)

## Both ran for the first time today

`mt-07` and `mt-26` have each produced exactly **one** verdict in nine cycles — cycle 9's — and both
broke. In every earlier cycle they "did not run": they sat in the truncated tail `qal-j28` describes,
and neither is desktop-tagged, so the desktop step never picked them up either.

So the half split did what it was for, and the first thing it surfaced was not a product fault but
two scenarios nobody had been able to exercise. **A scenario that never runs cannot be known to be
stale**, and 116 of them were in that position this morning.

## mt-07: optional fields read as required, arguments read as the brief

```
mt-07-brief-fields: briefs missing fields: [['do','read_first','report','rules'], [...]]
mt-07-read-first: read_first does not name project-context.md
```

The spawn call the model actually sent was `{"name", "output", "task"}`. Every field but `name` and
`task` is **optional** in the spawn schema, and `read_first` carries a default the kernel applies —
*"Paths to read first (default: project-context.md, notes.md)"* (`tools.rs:471`). The spawn tool's
own result shows what the worker got:

```
spawned write-river-poem: Write river poem
Read first: .arbos/docs/project-context.md, then .arbos/notes.md
Task: Write a 6-line poem about rivers to docs/river.md.
Do: as the task says; …
```

The property held completely. The scenario read `args` — the layer *before* the kernel renders the
brief — and concluded about what the worker was told.

Fixed to assert on the rendered brief: all three of `Read first:`, `Task:` and `Do:` present, and
`project-context` named. It passes on `cecd48e1bd76`, and the notes show why that is the fix rather
than a luckier run — the model sent six of seven fields that time but **still not `read_first`**, so
the old assertion would have broken on a passing run:

```
args_sent: [['do','name','output','report','rules','task'], […]]
briefs_rendered_complete: 2
read_first_in_rendered: [True, True]
```

This is the fourth layer fault today, after `qal-j34` (a height key that does not exist),
`qal-j36` (item data read to decide what the view draws) and `qal-j33` (typing at a window that was
not listening).

## mt-26: the assertion penalises the worker for obeying the brief

```
mt-26-context-read-twice: the child read project-context.md again although it was injected
```

Intermittent: `reread: True` on the first run, `reread: False` on the re-run, same staging, both with
`injected: True`. So it turns on whether the model chooses to read a file — which is a model choice
asserted as a product guarantee, review rule 6's shape.

And the choice is not arbitrary. The kernel **both** injects the project context **and** writes
`Read first: .arbos/docs/project-context.md` into the brief, as `mt-07`'s evidence above shows. A
worker that reads the file is doing what the brief told it to. The wasted call is real, but the
redundancy belongs to the kernel's brief, not to the worker's judgement.

**Left open deliberately, as a question rather than a fix**: should a brief name `read_first` for
content it has already injected? If not, the repair is in the brief and `mt-26` should assert on
the brief — that an injected path is not also listed as read-first. If the redundancy is intended
(belt and braces for a worker that ignores injected context), then `mt-26`'s claim cannot stand as
written and should assert the thing that matters, which is that the context arrives at all.

I have not changed `mt-26`. It is one intermittent red on a scenario whose first run was today, and
guessing at which side is wrong is how a stale assertion gets replaced by a differently stale one.

## Cross-references

- `qal-j28` — the truncation that kept both scenarios from ever running.
- `qal-j33`, `qal-j34`, `qal-j36` — the other three layer faults of the day.
