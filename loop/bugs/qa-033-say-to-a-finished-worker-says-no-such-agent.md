# qa-033: `say` to a worker that has finished (archived) answers "no agent is named X. Agents here: (none)"

- Feature: `say` tool / archive of finished workers (#144), `main` @ `12a07584`
- Severity: low-medium. Root did the right thing (steer the running worker) and was told the worker never existed; it then offered to spawn a new one. Kickoff item 7 ("steer a running worker mid-task") reads partial on every run where workers finish before the steer lands.
- Rollout: `internal/qa/rollouts/20260915T022221Z-kickoff-session/` (root transcript index 53-56)

## Repro

1. Coordinator root spawns `draft-design-for-file-system`; the worker finishes in a few seconds and is moved to `.arbos/archive/agents/`.
2. The user says: "New constraint for the design worker: … Do not restart it; steer it."
3. Root calls `say {mode: "steer", rename: "draft-design-for-file-system", text: …}`.

## Expected

Either the steer reaches the worker (if still running), or a message that says what happened: "`draft-design-for-file-system` finished at 02:23:10 (archived); its report is in `archive/agents/draft-design-for-file-system/`; spawn a new worker with the constraint, or edit the design yourself." Root can then act without guessing.

## Actual

`say: no agent is named "draft-design-for-file-system". Agents here: (none)` — the roster only lists live children, so a worker that existed a moment ago reads as never having existed, and the empty list is wrong for the user too (four workers ran).

## Suspected location

`crates/arbos-kernel/src/tools.rs` `say`: resolve the target against `archive/agents/` as well when it is not live, and answer with the finished-at time and the archive path. Also seen: the model put the target in `rename` rather than `to`; if `rename` is a real field of `say`, its description invites this.

## Fix

PR #213: `say` to an archived worker answers with the finished-at time, `.arbos/archive/agents/<id>/transcript.jsonl`, and what to do instead; an unknown name hears `Live agents: …` plus `N archived (finished): …`. `rename`'s description says it is not the target. E2e `say_archived_e2e`.
