# qal-j47 — a queued follow-up from the desktop stopped arriving, and the scenario reports it as a skip

- **status**: open (product), new on current `main`; a widening rate rather than a clean switch
- **found**: 2026-09-19 00:30, covering the desktop scenarios cycle 12 skipped for budget
- **control**: `sq-02-desktop-stop-holds-follow-up` — which **self-skips**, so the loop never flagged it
- **kernel**: fails on `55c8287765d7`; passes on `232518c26c1f`

## What happens

`sq-02` types a follow-up and queues it with ctrl/cmd-shift-enter while a turn runs. The kernel
should write an inbox file, which the window shows as a follow-up row under the composer. On
current `main` the row never appears, so the scenario gives up:

```
skipped: the follow-up could not be queued from the desktop (no inbox row after
         ctrl/cmd-shift-enter); the queued-follow-up path was not exercised
```

## It is the kernel, not the app

Measured by crossing the two. Same scenario, same harness:

| app | kernel | result |
|---|---|---|
| current (`55c82877` build) | `55c82877` | skip 4/4 |
| **older** (`01b32cd5`) | `55c82877` | skip 2/2 |
| current | **`232518c2`** (14:59) | **pass 2/2** |
| current | `1b4ef7a9` (19:31) | skip 1, **pass 1** |

The app makes no difference; the kernel does. And `sq-02` passed three times earlier today in the
cycle itself — 14:22, 16:41, 20:05 — on older kernels.

## A rate, not a switch

`1b4ef7a9` gives one of each, so this is not a commit that flipped a flag. It reads as a race that
recent kernel work widened: clean at 14:59, intermittent by 19:31, not once successful in six
attempts on tonight's build. **I am not pinning a commit on this evidence** — two runs at the
middle point is not enough to bisect a rate, and today has already taught me what happens when I
call a race deterministic from four samples (`qal-j43`).

The kernel-side commits in the window, for whoever picks it up:

```
4c7b938c  19:52  Merge #692: Terminal and Browser open promptly — the terminal pane reads the window's own attach
5b65c1cc  19:40  kernel: the open-speed harness takes its browser with it
27cf3ef9  18:56  loop: a folder question is one listing and then an answer
7cc8cc9d  18:56  loop: the Jev hop draws nothing on the chat face
aa88b654  18:50  kernel: a browser open leaves the frame loop
```

`#692` is the one that touches how the window's attach is read, and it merges directly on top of
`1b4ef7a9`, so it is where I would start — but I have not shown it.

## The rig half, which is the reason nobody saw this

`sq-02` reports this as a **skip**, not a break. A skip reads as "not applicable here" and is
counted with the scenarios that had no desktop at all. So a user-visible regression — a follow-up
you queued that is never queued — has been invisible in every cycle summary since it started.

This is the same shape as `kf-01`'s hollow pass, which I fixed this evening: an outcome that
proves a fault, recorded as an outcome that proves nothing. The rule the loop keeps relearning:

> If the scenario could not do the thing because the product would not let it, that is a finding,
> not a skip. A skip is for a rig that cannot ask the question, never for a product that will not
> answer it.

`sq-02` should break — or at minimum report a distinct outcome the summary counts — when the inbox
row does not appear. Left as it is, the fix landing would be just as invisible as the regression.

## Why it was found now

Cycle 12 skipped eighteen desktop scenarios for budget (`qal-j46`), `sq-02` among them. Re-running
those eighteen after the UTC reset is what surfaced this. Two of the eighteen found something: this,
and confirmation of `qal-j43`'s regression. That is a reasonable argument for `qal-j46`'s first
option — move the cheap, fragile desktop work earlier, so a budget that runs out at 22:00 does not
take it.
