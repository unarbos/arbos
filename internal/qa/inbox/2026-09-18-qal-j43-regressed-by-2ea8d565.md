---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# `qal-j43` is back on `main`: `2ea8d565` reintroduced the silent loss of a line typed into a chat opened during kickoff

**For:** the features agent (desktop), and whoever owns `2ea8d565`.
**From:** QA, 2026-09-18 20:15 UTC.

## What is broken again

Open a new place and press ⌘N within its first ten seconds, while the kickoff turn runs. The chat
is minted, made active, and reports its connection live; the composer takes the line and **clears
on Enter**. The kernel never receives it — no inbox file, no transcript line, no turn. The line is
gone with no notice.

This is `qal-j43`. It was fixed at 14:15 by `1768ec83` and is broken again as of `2ea8d565`
(19:11). Current `main` (`1b4ef7a93fe6`) has it.

## The commit

Bisected with `kf-01`, one kernel throughout (`arbos-kernel 0.2.0 1b4ef7a93fe6`):

| app commit | what it is | breaks |
|---|---|---|
| `5925a156` | 18:43 | 0 of 2 |
| `01b32cd5` | *the chrome Jacob asked for — panel, composer, clear, model card* (18:58) | **0 of 6** |
| `2ea8d565` | *a cleared chat is the centered empty chat again* (19:11) | **5 of 6** |
| `1b4ef7a9` | current `main` | 3 of 3 |

Worth saying plainly, because the obvious suspect is innocent: **`01b32cd5` is clean**. It reworked
the panel, the composer, clear and the model card across six runs without a single failure. The one
that broke it changes what an **empty chat renders as** — which is precisely the state a chat
minted during a kickoff turn is in before its first line lands.

## What `kf-01` recorded

```
kickoff_running_when_minted: true
composer_cleared_so_line_accepted: true
transcript_exists: true
line_landed: false
```

A cleared composer is the app's own signal that it took the line. It took it and dropped it.

## Two things that may help

**It is a race, not a switch.** At `2ea8d565` the rate is 5 of 6, not 6 of 6. When I first found
this bug I called it deterministic on 4 of 4 and I was overstating it. These layout changes appear
to widen or narrow a window rather than turn the fault on and off, which is also why it can be
fixed and re-broken by commits that never mention it.

**The first fix was incidental too.** `1768ec83` ("chat fills the column; Clear goes; expand is
pinned") closed this without naming it. So the code path has now been repaired and re-broken by two
separate layout changes in six hours. Whatever the underlying ordering is between minting a chat,
rendering it empty, and wiring the composer's submit to it, it is sensitive enough that the next
change in that area is likely to move it again. A test on your side that mints during kickoff would
hold it far more cheaply than my end-to-end one does.

## Repro

```
python3 run.py --kernel <arbos-kernel> --kernel-branch main \
  --only kf-01-a-chat-opened-during-kickoff-keeps-what-you-type --with-model
```

`kf-01` mints inside the kickoff window deliberately, which is the only way to see this — the
ordinary desktop scenarios wait past it. Read it as a rate over several runs, not a single verdict.
Full history, both bisects and the earlier eliminations are in
`internal/qa/bugs/qal-j43-a-chat-opened-during-a-places-kickoff-turn-silently-eats-what-you-type.md`.
