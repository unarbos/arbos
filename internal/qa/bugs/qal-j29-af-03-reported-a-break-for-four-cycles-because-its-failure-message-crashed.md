# qal-j29 — af-03 reported a break for four cycles because its failure message crashed

- **status**: fixed in the rig (this loop's own defect, not a product defect)
- **found**: 2026-09-18 07:08, cycle 6 desktop step
- **kernel**: `arbos-kernel 0.2.0 d373422662bd protocol 1`; app at `b1c8e82a62b1`
- **rollout**: `20260918T065516Z-af-03-desktop-folder-renamed-under-the-window`
- **rule it printed**: `driver-exception: IndexError: list index out of range`

## What happened

`af-03-desktop-folder-renamed-under-the-window` ends with three assertions. The third was:

```python
wrong = [n for n in notices if "archived" in n.lower() and not any(...)]
cx.rec.expect(not wrong, "af-03-wrong-explanation", f"... as {wrong[0]!r} — nothing was archived; ...")
```

Python builds the f-string argument **before** `expect` is called, so `wrong[0]` is evaluated
whether or not `wrong` is empty. `wrong` empty is the **passing** case. So the assertion crashed
precisely when it succeeded, and the scenario reported `driver-exception` instead of a pass.

It fired in four consecutive cycles — 2026-09-17 18:18, 20:55, 2026-09-18 01:00 and 04:28 — and
in none of them did anyone learn what `af-03` had found.

## What it was hiding: the product is correct

The notes were written before the crash, so the observation survives in every one of those four
rollouts. From this cycle:

| note | value |
|---|---|
| `ghost_at_old_path` | `False` |
| `line_in_moved` | `False` |
| `line_in_ghost` | `False` |
| `connection` | `'lost'` |
| notice shown | `This project's folder is gone or was moved: expected /tmp/arbos-qa-af-03-desktop…` |

All three assertions pass on that: no ghost `.arbos/` was minted at the old path, so the
destructive history-splitting case `af-03` exists to catch **does not happen**; the chat says
something rather than nothing; and what it says is true — it names a moved folder and does not
claim anything was archived.

So the desktop's handling of a folder renamed under an open window is sound on this build, and a
rig fault has been reporting it as broken since yesterday evening.

## The fix

```python
explained = repr(wrong[0]) if wrong else "nothing"
cx.rec.expect(not wrong, "af-03-wrong-explanation", f"... as {explained} — ...")
```

Verified by evaluating both shapes against `wrong = []`: the old one raises `IndexError`, the new
one renders `as nothing —`. Synced to the store at `internal/qa/landing_scenarios.py`.

## Sweep

Every `.expect(` in the library was checked for an index or `.pop(` inside the message argument.
Four sites matched; three are already safe — `landing_scenarios.py:141` and `:770` guard with
`evs[-1]… if evs else None`, `:1038` guards with `rounds[0]… if rounds else None`, and `:139`'s
`[0]` is on a `.split()`, which always returns at least one element. `af-03` was the only
unguarded one, and it is fixed.

## The rule this belongs to

This is the sixth review rule — **an assertion must not bound a race** — read from its other end:
that rule asks whether a *failure* would mean anything. Here the failure meant nothing at all,
because the failure path could not even be reached without raising first. A break whose rule name
is `driver-exception` is worth treating as a rig fault until proven otherwise, and worth treating
as **urgent**, because unlike a product break it hides its own scenario's finding.

The general form worth adding to the review list: **a failure message must be computable when the
assertion passes.** Anything the message indexes, pops or unwraps has to be safe in the passing
case, because Python computes it first.
