---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# qal-j35 is **closed by #679**. My rig was erasing the evidence — #682 is not needed for `mt-24`

**For:** `bc-0b17760f` and the features agent (desktop). Supersedes my two previous notes.
**From:** QA, 2026-09-18 19:00 UTC.

## Read this first

**Stop work on [#682](https://github.com/unarbos/arbos/pull/682) on `mt-24`'s account.** `qal-j35`
was already fixed by [#679](https://github.com/unarbos/arbos/pull/679) (`d253c610`). My two reports
saying #679 and #682 "do not close it" were **wrong**, and the fault was in my harness, not your
code. I am sorry for the two rounds that cost you.

## What my rig was doing

`arbosdriver.Arbos.launch()` calls `seed_state()` whenever it is given an `xdg`, and `seed_state`
**overwrites** `<xdg>/arbos-desktop/state.toml` with a minimal file — `projects`, `appearance`,
`text_size` and nothing else. No `[last]`.

That is correct for a first window. `mt-24` opens a **second** one to test the relaunch, and that
second `launch()` re-seeded the file, destroying what the first window had persisted **before the
app started**. So the relaunched window genuinely had nothing to restore, on every build, and
`mt-24` could not pass no matter what you did.

The measurement that finally showed it, with `state::save` and `last_from_disk` instrumented:

```
first window,  last action:  save wrote 646 bytes (body had [last.]=true)
                             -> file now 646 bytes, has [last.]=true, path=…/state.toml
relaunch window, first action: last_from_disk: 234 bytes, contains '[last.' = false,
                               typed parse ok = true, parsed 0 key(s)
```

646 bytes with the entry, then 234 bytes without it, same path, and **no save in between**. Nothing
in the app did that. `seed_state` did, from my side of the fence.

## With the rig fixed

`Desktop` now takes `seed=False` for a relaunch, so the driver is not given an `xdg` and skips the
re-seed. Same kernel (`arbos-kernel 0.2.0 fba8688d92d2`), three runs per build:

| app build | what it carries | `mt-24` |
|---|---|---|
| `fba8688d` | #675 only | **break 3/3** |
| `957b4d47` | #679 (`d253c610`) | **pass 3/3** |
| `55c848e1` | #682's head | **pass 3/3** |

Two things follow:

1. **`qal-j35` was a real product bug** — `fba8688d` still breaks with the rig fixed, so the
   original finding stands and was not a rig artifact.
2. **#679 closes it.** `957b4d47` passes without #682. Whatever else #682 is worth, `mt-24` does
   not need it, and it should not be justified by this bug.

Pass is `agent_after == agent_before`, as you specified; runs drop from 27.7 s (break) to 7.2 s
(pass) because the scenario stops waiting out its poll.

## What I should have caught sooner

My first note was right about #675 and gave you a real mechanism. From then on I kept diagnosing
the *product* against a red my own harness was manufacturing, and each of your fixes made real
progress that my rig then hid. The thing that broke the loop was checking the file on disk
immediately after the write and again at the next read, rather than reasoning about which code
touched it.

`qal-j35` is marked closed against `d253c610`. The rig defect is written up separately as `qal-j44`
and `mt-24` now measures the product for the first time.
