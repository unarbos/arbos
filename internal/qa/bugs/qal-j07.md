# qal-j07: on #377, a shell subscription whose command backgrounds a child is held for the child's life — its reading never arrives

- Feature: `kind = shell` subscriptions run by the kernel (the cron), under #377's job leash (a signal at the displayed pid ends the whole group; the wrapper watches its parent). Branch `cursor/…` at `44dcc70d`, which already contains #364.
- Severity: high for the visible user path #377 does not touch: a subscription like `start the dev server in the background and report` fires, the command exits at once, and nothing is delivered — the subscription sits `last_fired` with no reading, its next due time passes unfired, for as long as the backgrounded child lives (here `sleep 300`; for a server, for ever). On the current `main` kernel the same reading arrives in 0.4 s. #377's author named this cost and said it needs #364 beside it; #364 is in #377's base and the hold is still there, so the pair does not close it.
- Scenario: `sb-01-backgrounding-subscription-command-finishes`; rollouts `internal/qa/rollouts/20260917T031324Z-sb-01-…` (45 s, nothing) and `20260917T031439Z-sb-01-…` (200 s, nothing; `last_fired` set, `next_due` passed). Control on `main` `5017ef45`: `20260917T031416Z-sb-01-…`, reading in 0.4 s.

## Repro

`.arbos/agents/root/subscriptions/0001-bg.toml`:

```
kind = "shell"
every = "30s"
cmd = "nohup sleep 300 >/dev/null 2>&1 & echo started-bg"
deliver_to = "user"
notify = "bg: {output}"
next_due = <now − 1 s>
```

Start the kernel, attach, wait for a frame carrying `bg: started-bg` (not the snapshot that echoes the file). `main`: 0.4 s. #377: none in 200 s; the file reads `last_fired = 03:14:39Z`, `next_due = 03:15:09Z` (passed, not fired again — held by name).

## Expected

The command exits immediately (`echo started-bg` after backgrounding); the run completes, `bg: started-bg` reaches the user within seconds, and the backgrounded child is leashed and capped without holding the run.

## Actual

The run waits on the process group, which the backgrounded child keeps alive; the reading is held for the child's lifetime; the subscription's next fire is blocked by the in-flight run.

## Suspected location

- `crates/arbos-kernel/src/plan.rs` shell run under #377: waiting for the group (or the wrapper waiting for all descendants) rather than for the command's own exit; the `exit` file is written when the command exits and could be the signal, with the leash keeping the child capped afterwards.

## Fix

Not started (with the features agent; #377 pre-merge). Regression check: `sb-01` (reading within 45 s; `ARBOS_QA_SB01_WAIT` widens the wait for diagnosis).
