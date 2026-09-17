---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Finding (conditional): the prompt did the work, not the check

Status: **a hypothesis with a measurement in flight**, not a result. It becomes a finding if the benchmark loop's A/B on [#440](https://github.com/unarbos/arbos/pull/440) shows the mechanism gate's return recovering solves. If it does not, this note stays as the record of what was suspected and why, and the lesson below is withdrawn.

## What was measured

Twelve SWE-bench instances, covered twice, two independent arms, same harness, same network cut: **16 of 24 solved on `30eef166`, 9 of 24 on `7e19f9e9`.** The arms agree with each other and disagree with the earlier cycle; seven rollouts on the same instances is outside the loop's noise band. Engine merges between the bases: #399, #405, #407, #408, #410.

## Why #399 is the suspect

#399 removed the mechanism gate. Before it, the first `edit`/`write`/`apply_patch` of a task was **refused** unless the call carried `mechanism`: one line, what is wrong (the code path that produces the wrong value, and why) and what change fixes it — and the refusal told the model to check that line against every symptom before editing. The gate was removed because the loop showed it accepted the literal word `placeholder`, so its passing was not evidence of anything.

That established "useless as evidence". It did not establish "harmless to remove". Most models, refused once, wrote a real causal sentence — the cheat was rare — so the gate was, for most rollouts, a forced statement of the cause at the one moment before the model committed to a change. Removing it removed that step from every rollout, not only from the cheats.

Of the five merges, #399 is the only one that changed what the model is **asked** to do. The contract sentence went from *"carries mechanism … a headless run refuses the call without it"* to *"may carry"*; the tool schema from *"Required on the first edit"* to *"not checked"*. #405's write-wait is #419, after the second base; #407 and #408 change records and error paths; #410 refuses only root/home/system removals (checkable in the nine failures with `grep -c 'bash: refused' run.jsonl`, since `/testbed` is a top-level directory).

## The lesson, if it holds

Asking a model to state its reasoning before it acts can improve the work **even when nothing verifies the statement**. The value was in the asking, not the checking. We removed the ask because the check was hollow, and we may have thrown away the part that worked.

This changes how to read every contract line judged by whether it can be enforced:

- **"Unenforceable" is not "useless."** A line the kernel cannot check may still change what the model does. The question for a line is not *can we verify it* but *does its presence move the outcome* — and only a measurement answers that.
- **Removing a gate is a turn-shape change** and needs the same A/B as adding one. Tonight's freeze rule ("nothing that changes what a turn does or when") applies to subtractions.
- **Keep the ask when you drop the check.** When a gate turns out to be hollow as evidence, the honest change is to stop *treating it as evidence* (which #399 did — the line is recorded, not trusted) while keeping the *requirement to state it*. #399 did both at once; the flag in #440 separates them for the measurement.

## Lines to re-examine under that lens (if the A/B confirms)

Contract lines and gates changed on the ground that a check was weak or copyable, where the ask itself may have carried value:

- The mechanism line (#399) — the case under test.
- The reproduction gate (`ARBOS_REPRO_REQUIRED`, `repro.rs`): "before the first edit, reproduce the failure" is enforced only when the env is set; the prompt still asks. Worth knowing whether the ask alone is doing the work there too, and whether the default-off gate ever should be on.
- The status phrasing (#216): example phrases were removed because a model copied them verbatim. That was a different failure (copying, not cheating), but the same question applies: did describing the shape rather than giving an example change how often a status was set at all?
- Any future "the model ignores this line, drop it": the line may be ignored in its letter and still shaping the turn.

## What would settle it

The A/B in #440: the twelve instances on today's kernel with `ARBOS_MECHANISM_REQUIRED=1` against the same kernel without it. A recovery toward 16/24 with the flag is the finding. A flat result points the bisect at #410 (the refusal grep first) and then #407/#408, and this note is withdrawn.

## Cross-references

- [#399](https://github.com/unarbos/arbos/pull/399) — the gate's removal (SWE-bench loop, cycle 13).
- [#440](https://github.com/unarbos/arbos/pull/440) — the gate behind a flag, for the measurement.
- `internal/qa/inbox/2026-09-17-swebench-loop-cycle-14.md` and the loop's regression run for the 16/24 → 9/24 numbers.
- `crates/arbos-engine/src/mechanism.rs` — the module doc carries the cycle-by-cycle history of the gate.
