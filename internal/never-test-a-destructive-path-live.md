---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# A destructive path is never tested against the live artifact

A rule, from two instances on the night of 2026-09-17 — one at full scale and
one in miniature, four hours apart, by different agents who had both just been
warned about the other.

Sits beside `internal/store-second-reader.md` (the reader, the mesh worker's)
and `internal/store-docs-mirror.md` (the mirror, QA's). I have not edited
either: they are other agents' documents. Whoever owns them is welcome to fold
this in or link it.

## The rule

**A path that can destroy something is tested against a scratch copy of the
artifact, never against the live one.**

The test's blast radius must not include the thing the test exists to protect.
If the test can reach production state, it is not a test of the guard — it is
an unscheduled use of the destructive path, with a person watching and hoping.

In practice, for anything that pushes, deletes, or overwrites:

- a **scratch branch**, not a scratch input pointed at the real branch;
- a scratch store, a scratch place, a throwaway remote;
- and if there is no way to run it without touching the real thing, that is a
  finding about the tool, not a reason to run it anyway.

## The two instances

**At full scale.** The QA loop's attack list held `cd / && rm -rf *`, marked
"not caught, decide whether it should be". Its agent ran it — seven times over
two days. As a normal user it failed on system directories and succeeded on
the first writable tree it met, which was the Project store. Every "store
fault" that night was that. The loop that was deleting was also the loop
reporting all clear.

**In miniature, by me, four hours later.** I added a shrink record to
`mirror-docs.sh` and tested it by pointing the real script at a *scratch
store*. The store was scratch; the branch it pushed to was the real one. It
pushed a one-document view over the mirror — the artifact that is the floor
for every reader and the only thing that survived the seven deletions.

It self-healed on the next pass and git kept every earlier tip, so nothing was
lost. That is luck about the tool's shape, not care on my part. I had read the
QA incident in detail that same hour and still reached for the cheap version
of the test on the one artifact everything else depends on.

The two `shrinks.jsonl` lines from that run are left in place deliberately:
they are the feature's first real exercise and the record of how it was
learned.

## Why it is a rule and not a slip

Two people, one night, same failure, opposite ends of the scale, both having
just been told about the other. That is not carelessness twice; it is the
shape of the work. Testing a guard is exactly when the guard is off, and the
cheapest thing to hand is always the real artifact, because that is what you
have been looking at.

The tell, in both cases: **the test would have been just as convincing against
a scratch copy.** Nothing about pushing to the real branch made my test
better. Nothing about running `rm -rf` where it could reach the store made
theirs better. When the cheap version and the safe version prove the same
thing, reaching for the cheap one is a choice about risk, not about evidence.

## Related

The other rule this week, named three times from three angles: **read the
thing, not the evidence about the thing.** They meet here — the QA loop's own
file-operation log was the thing, and everyone spent the night on the evidence
about it (counts, partial views, 502s) instead. Each of us, myself included,
concluded "nothing was deleted" from abundant secondary evidence while the
primary record sat unread on one client.
