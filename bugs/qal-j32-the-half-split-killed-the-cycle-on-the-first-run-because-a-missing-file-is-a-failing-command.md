# qal-j32 — the half split killed the cycle on its first run, because a missing file is a failing command

- **status**: fixed, with a control
- **found**: 2026-09-18 08:35, cycle 7
- **cause**: mine — the library half split added to `deploy/cycle.sh` in cycle 6
- **cost**: cycle 7's tracked step, step 3a2, the desktop step, the acceptance journey and the mirror

## What happened

The half split remembers which half ran last in `state/library-half` and flips it:

```bash
HALF_FILE="$ROOT/state/library-half"
LIB_HALF=$(cat "$HALF_FILE" 2>/dev/null)          # ← dies here on the first cycle
case "$LIB_HALF" in A) LIB_HALF=B;; B) LIB_HALF=A;; *) LIB_HALF=A;; esac
```

`cycle.sh` runs under `set -euo pipefail`. A command substitution takes the status of the command
inside it, so when the file does not exist `cat` exits 1, the assignment exits 1, and the script
ends. The `*)` branch was written precisely for the first-cycle case and could never be reached.

`2>/dev/null` made it worse: it hid the one line that would have named the cause, so the log shows
the untracked step finishing normally and then nothing. From outside it looked like the cycle
simply stopped.

Cycle 7 died at 08:33, between the untracked step and the tracked one, and `vm-loop` recorded
`== cycle failed (1)` and scheduled the next for 09:00.

## The fix

```bash
mkdir -p "$ROOT/state"
LIB_HALF=$(cat "$HALF_FILE" 2>/dev/null || true)
```

**Control**, on the exact lines: with no state file the first run prints
`== library half this cycle: A` and exits 0, the second prints `B`, the third `A`. Before the fix
the first run printed nothing and exited 1. Swept the rest of `cycle.sh` for the same shape —
one other `VAR=$(…)` assignment, `SHA=$(git rev-parse --short=12 HEAD)`, which should stop the
cycle if it fails and is left alone.

## What this is an instance of

I added an alternation that needs state, and tested the steady state rather than the first run.
The branch handling the absent file was there, was correct, and was unreachable — the failure came
before the value was ever examined.

It also belongs beside `qal-j29` from earlier today: both are a line that behaves differently in
the case nobody exercised, and in both the *quiet* path was the broken one. `qal-j29` raised an
exception when its assertion passed; this raised one when its state file was absent. Review rule 8
covers the first. The general form covering both:

**Run the path that has no history.** A first run, an empty directory, a missing file, a passing
assertion — the states where there is nothing yet are the ones written from imagination rather
than from observation.

## Recovered by hand

Step 3a2's set was run manually at 08:37 against `arbos-kernel 0.2.0 d373422662bd protocol 1` so
the cycle's loss was not total: `af-04` **passed** (79.8 s, `old_path_recreated: None`, a turn
completed and a new kernel served the moved folder), `uw-01` through `uw-04` all passed, and
`fm-02` broke as filed in `qal-j31`. Six run, none skipped.
