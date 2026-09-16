# qal-j02: a kernel that dies mid-tool leaves no record of the tool, so the continued turn runs the command again — a side effect happens twice

- Feature: turn continuation after a kernel restart (`needs_serve()` refires the open `wake`; the model sees a turn with no tool record and calls the tool again); `main` @ `c964294c`, model `google/gemini-2.5-flash`
- Severity: high for anything non-idempotent — a `git push`, a `curl -X POST`, an `rm`, an `echo >>`, a payment. Harmless for reads and for `git commit` ("nothing to commit"). The features agent found this while walking the journey (2026-09-16) and left it unfixed on purpose as "worth watching"; the journey's J8a now watches it and it doubled on the first try.
- Journey step: **J8a (kernel restart mid-turn)**. Scenario `journey-linux`; rollout `internal/qa/rollouts/20260916T134759Z-journey-linux/` (evidence `J8.restart.side_effect_runs = 2`). Related e2e: #316 pins the restart itself.

## Repro

Desktop app on a project, kernel serving. Send:

> Run exactly this with bash and nothing else first: `echo ran-J79175 >> side-effects.log; sleep 30; echo waited`. Then say done.

Wait until `side-effects.log` exists (the echo ran), then `kill -9` the kernel during the sleep. The desktop respawns it; the open turn continues. Root's transcript afterwards:

```
55 wake       Run exactly this with bash …
56 user       Run exactly this with bash …
57 wake       (empty — the continued turn)
58 assistant
59 tool bash  {"command": "echo ran-J79175 >> side-effects.log; sleep 30; echo waited"}
60 assistant  Done.
61 turn_complete
```

`side-effects.log` holds `ran-J79175` **twice**. Nothing between 56 and 57 says a bash call was in flight when the kernel died.

## Expected

The user sees the command run once. Either the in-flight tool call is on the transcript before it runs (a `tool` line with the call and no result yet, or a `tool_started` marker), so the continued turn's model sees "this ran; the kernel died before its result" and reports/asks instead of re-running; or the continued turn is told plainly "the kernel restarted during `bash …`; the command may have run — check before repeating it." For a command the kernel can prove idempotent (reads), re-running silently is fine.

## Actual

The `tool` line is written only when the tool returns. A kernel killed mid-tool leaves the transcript at the `user` line; the continued turn is a fresh model step with the same prompt and no memory of the call, so it calls the tool again. Every non-idempotent command in flight at a kernel death runs twice.

## Suspected location

- `crates/arbos-engine/src/turn.rs`: the tool call is appended after `tool.run()` resolves. Append a `tool_call`/started record before running (with `call_id`, name, args) and let the result line complete it; on continuation, an unfinished started record becomes a notice into the model's context.
- `crates/arbos-kernel/src/serve.rs` / `hooks.rs`: the continuation path (`needs_serve()` → refire) is where the "kernel restarted mid-tool" line would be injected.

## Fix

[#316](https://github.com/unarbos/arbos/pull/316) (features agent, 2026-09-16): each running tool call is a file under `agents/<id>/inflight/` from start to result; at boot the kernel writes any leftover as a `tool` line — `error: "interrupted: the kernel restarted while this ran"`, body telling the model what ran, for how long, that it may have completed, and to check before repeating. `restart_mid_turn_e2e` now runs `echo ran-J8A >> side-effects.log; sleep 8` across a SIGKILL and asserts one line, one bash record, the cut record before the serve wake. `J8.restart.side_effect_runs` should read 1 once #316 is on `main`. Regression check: `journey-linux` J8a (`side_effect_runs` must be 1) and an e2e beside #316's: replay provider, a `bash` that appends to a file and sleeps, `SIGKILL` the kernel mid-sleep, restart, assert one line in the file and a transcript line that names the interrupted call.
