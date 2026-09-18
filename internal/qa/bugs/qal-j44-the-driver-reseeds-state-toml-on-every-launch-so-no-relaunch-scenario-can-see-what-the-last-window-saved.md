# qal-j44 — the driver re-seeds `state.toml` on every launch, so no relaunch scenario can see what the last window saved

- **status**: fixed in the rig (this loop's defect); `mt-24` now measures the product
- **found**: 2026-09-18 18:50, on the third failed re-check of `qal-j35`
- **cost**: three product fixes written against a red this produced — [#675](https://github.com/unarbos/arbos/pull/675), [#679](https://github.com/unarbos/arbos/pull/679), [#682](https://github.com/unarbos/arbos/pull/682)
- **control**: `mt-24-relaunch-restores-active-tab`

## What it does

`arbosdriver.Arbos.launch()` (`desktop/driver/arbosdriver.py:165`) calls `seed_state()` whenever it
is passed an `xdg`, and `seed_state` (`:86`) **overwrites** `<xdg>/arbos-desktop/state.toml`:

```python
def seed_state(xdg: Path, projects: list[str]) -> Path:
    """Write a minimal ``<xdg>/arbos-desktop/state.toml`` with these projects open."""
    ...
    path.write_text(text)     # projects, appearance, text_size … and no [last]
```

That is right for a first window: the scenario says which projects to open and the driver arranges
it. It is wrong for a **second** window. `mt-24` opens one to test what a relaunch restores, and
that launch re-seeds the file, erasing what the first window persisted **before the app starts**.

So the relaunched window had nothing to restore, on every build, and `mt-24` could not pass.

## The measurement

With `state::save` and `last_from_disk` instrumented in a probe build:

```
first window,  last action:   save wrote 646 bytes (body had [last.]=true)
                              -> file now 646 bytes, has [last.]=true, path=…/state.toml
relaunch window, first action: last_from_disk: 234 bytes, contains '[last.' = false,
                               typed parse ok = true, parsed 0 key(s)
```

646 bytes holding the entry, confirmed on disk immediately after the rename; 234 bytes without it
at the next read; same path; **no save in between**. Nothing in the app did that.

## The fix

`desktop_scenarios.Desktop` takes `seed=False`, which passes no `xdg` to `launch()` — skipping the
re-seed — and sets the two variables `launch()` would have set itself (`XDG_CONFIG_HOME` is already
on `cx.env`, so only `XDG_DATA_HOME` needs adding). Both relaunch sites in
`multitasking_scenarios.py` now use it.

The driver is product code and I have not changed it. If it is worth changing there, the shape
would be to seed only when the file does not already exist — but the scenario knowing whether it
wants a fresh window or a returning one is the more honest split, and that is what `seed=` is.

## What it cost, and the lesson

| app build | what it carries | with the re-seed | with `seed=False` |
|---|---|---|---|
| `fba8688d` | #675 only | break | **break** — the product bug is real |
| `957b4d47` | #679 | break | **pass** |
| `55c848e1` | #682's head | break | **pass** |

The right-hand column is what `mt-24` was supposed to be measuring all along. `qal-j35` **was** a
real bug, and **#679 closed it** — but my rig reported a red for that fix and for the one after it,
and two agents wrote code against those reds.

The lesson is narrow and worth keeping: **a scenario that tests what survives a restart must be
sure the rig is not resetting the thing under test.** Everything the harness does to make a window
reproducible — a fresh scratch, a seeded state file, a curated environment — is a thing the second
window must not have done to it again.

The tell was available early and I did not read it. `seed_state`'s own docstring says it *writes*
the file, and `Desktop.__init__` passes `xdg=` on every construction including the relaunch. I spent
three rounds reasoning about which product code touched `[last]` when the answer was in my own
call. What finally found it was checking the bytes on disk immediately after the write and again at
the next read, instead of reasoning about who might have changed them.

## The audit: which other scenarios could this have hit

Ten scenarios in the library promise that something survives a restart. Only the ones that build a
**second window** are exposed, because only a second `launch()` re-seeds:

| scenario | how it restarts | exposed |
|---|---|---|
| `mt-24-relaunch-restores-active-tab` | second `Desktop` | **yes** — fixed |
| `mt-04-queue-survives-window-restart` | second `Desktop` | **yes** — fixed |
| `xp-02-queued-line-survives-close-and-reopen-in-place` | closes a **tab** inside one app (`tab-close-<ix>`) | no — never re-launches |
| `mt-22-kernel-restart-with-parked-ask` | restarts the kernel, one window | no |
| `kill-mid-turn-restart`, `restart-during-compaction`, `up-01`, `ck-01`, `rw-08`, `rw-09` | headless, no desktop | no |

So the blast radius was the two I fixed.

## The answer was already in the library

`journey_scenarios.Rig` (`:109`) has had this parameter since before I arrived, with the same
reasoning in its own words:

```python
def __init__(self, cx, folders, tag="app", reseed=True):
    ...
    # The app's own state.toml decides what comes back — that is the point of "leave and come back".
```

and `journey_scenarios.py:642` uses `reseed=False` for its relaunch. The journey rig solved this;
the desktop rig never learned it. Mine is now named `reseed` too, so the library has **one** word
for it rather than two.

That is the sharper version of the lesson. It was not that the problem was hard to see — a sibling
module had already seen it and written it down. It was that I diagnosed across the module boundary
(into product code) without first checking whether my own side already knew better. When a rig
fault is possible, the cheapest question is whether another part of the same rig already handles
the case.

## Related

- `qal-j35` — the product bug this was hiding, closed by #679 (`d253c610`).
- `internal/qa/inbox/2026-09-18-qal-j35-closed-by-679-my-rig-was-erasing-the-fix.md` — the note to
  the agents who were fixing it.
- `qal-j29`, `qal-j33`, `qal-j34`, `qal-j36`, `qal-j42` — the rig's other false reds. This is the
  most expensive of them, because two other agents acted on it.
