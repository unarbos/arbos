# qal-j21: `publish.sh push` mirrors this machine's bug folder onto `qa-results` with no floor, so a loop standing up on a second machine deletes every draft the branch holds and says "pushed"

- Measured at: the loop's own `deploy/publish.sh` as the store held it at 2026-09-17 12:31 (`internal/qa/deploy/publish.sh`, blob `d4cc4038`-era tree; the file itself unchanged since 07:21), driven by `pb-01` against a scratch remote. Not a kernel bug: this is our tooling, and it was found while standing the loop up on a second machine rather than by reading.
- Class: destructive, with a success message — the shape of qal-j09 and qal-j13 in our own scripts. A mirror with no floor, next to a store mirror that has had one since 2026-09-16 ("the mirror never accepts a smaller tree without a reason").
- Feature: `deploy/publish.sh push`, the `qa-results` branch.

## What happens

`push` is a mirror: `rm -rf $RESULTS/bugs && cp -r $ROOT/loop/bugs/.`. It therefore publishes exactly what this machine holds, and deletes whatever it does not.

A machine standing the loop up seeds `loop/bugs` from the store, and `vm-loop.sh` seeds only the curated files:

```
cp -f "$STORE"/bugs/qa-*.md "$STORE"/bugs/qal-*.md "$ROOT/loop/bugs/"
```

Measured on this machine at 12:31 UTC: that gives **75** files, where `qa-results` carries **208** (130 of them auto-drafts, `bugs/<fingerprint>.md`). The first cycle's publish would have removed **133** of them. `publish.sh`'s whole output for that is one line:

```
-- publish: pushed
```

The branch's set is a subset of the store's, so nothing would have been lost for ever — but the published record of 130 drafts would have gone, and the next `publish.sh pull` does not bring bug files back (it copies `inbox/` only), so no machine would have restored them.

Driven, rather than argued: `pb-01` builds a scratch remote whose `qa-results` holds three drafts and one curated bug, a loop tree holding the curated bug and one new finding, and runs `push`.

- Control (the store's script, with only the guard removed so the two differ by the guard alone): exit 0, `-- publish: pushed`, the branch goes from 4 files to 2 — the three drafts gone, the new finding arrived. It looks like it worked.
- Fixed: exit 4, `!! PUBLISH REFUSED: 3 bug file(s) on qa-results are not in …/loop/bugs; pushing would delete them.`, branch unchanged.

## What we expect

Two changes, both made:

1. **`publish.sh` refuses a push that removes bug files** unless `ARBOS_QA_ALLOW_BUGS_SHRINK='<reason>'` is set, which it then prints. Folding a draft into a curated file is still a removal; it just has to be said. (`REMOTE` also became overridable, `ARBOS_QA_RESULTS_REMOTE`, so the destructive path can be driven against a scratch remote instead of the live branch.)
2. **`vm-loop.sh` seeds the whole set on a cold start**: `cp -n "$STORE"/bugs/*.md` beside the curated `cp -f`, never over a local copy, which may carry `seen` lines the store has not taken.

## Regression check

`pb-01` (`/tmp`-built, to go beside the loop's deploy scripts): passes when the branch keeps every file it had while this machine's tree lacked some of them, and reports `SKIP probe-staged-no-divergence` when the two sets happen to agree, so it can never pass for the wrong reason.

Two things the first version of that probe taught, both instances of "ask what else could make it pass":

- Its first control passed because the unguarded script died on a missing `loop/rollouts` before it ever pushed. A loop tree always has one; the probe's world was wrong.
- Its second control passed because `REMOTE` was hardcoded in the store's copy, so the override did nothing and the script reached for the **live** `qa-results`; only a wrong `ARBOS_GITHUB` stopped it. Never test a destructive path against the live artifact — and a control must differ from the fix by the fix alone.

## Why it had not been met before

One machine has run this loop all day, and its `loop/bugs` is where the drafts are written, so its tree was always the superset. The fault needs a second machine, which is exactly what a handover is.
