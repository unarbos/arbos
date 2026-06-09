# ADR-0032 — Background jobs: the filesystem is the contract

- Status: Accepted
- Date: 2026-06-09

## Context

pi's bash tool was synchronous: it waited for the command and SIGKILLed the
process group on timeout (120s default). For a long-running agent that is the
wrong default twice over — a slow build is *killed* rather than finished, and
nothing the agent starts can outlive a turn, let alone the arbos process. The
competitive bar (Cursor's background shells, notify-on-regex, await) keeps the
processes inside the IDE host, so they die with it. Arbos's thesis is durable,
multi-day work; its process management has to survive the agent itself.

## Decision

Every bash command runs as a **journaled job** whose source of truth is a
directory on disk, never process memory. The contract
(`internal/tool/codingspec/job.go`):

```
$TMPDIR/arbos-jobs/<sha256(workspace-root)[:12]>/<id>/
  meta.json   command, cwd, pid, startedAt — written once at spawn
  out.log     combined stdout+stderr — written by the child's own fds
  exit        "<code>\n" — written by the wrapper subshell at completion
```

- **The child journals itself.** Its stdout/stderr *are* the `out.log` file
  descriptors; output is never piped through arbos. "Streaming" is reading a
  file, and an arbos crash loses nothing.
- **The child records its own exit code.** The command runs as
  `( cd <root> && <cmd> ); echo $? > exit` — the subshell isolates `exit`
  statements and the wrapper persists the code with no waiting parent needed.
- **Status is derived, never stored.** `exit` file present → exited (read the
  code; the file's mtime is the end time). Else pid alive → running. Else →
  killed (group SIGKILL or reboot). Every reader — foreground bash, `await`,
  `jobs`, the turn-start injector, a delegated child, a restarted arbos —
  derives status from the same three files through the same code path
  (`loadJob`/`listJobs`).
- **The directory is shared by design.** Keying the jobs root by workspace
  (not by process or session) is the contract that makes restart re-adoption
  and parent/child visibility work: a brand-new arbos process lists, awaits,
  and recovers the exit code of a job it never owned. Concurrent writers
  cannot collide on ids (`j1`, `j2`, …) because allocation is an exclusive
  `Mkdir`.
- **The in-process supervisor is only a cache**: live cmd handles for zombie
  reaping and the race-guarded group kill, per-job read offsets so tools
  return incremental output, and completion notices for backgrounded jobs
  (appended to the next bash/await/jobs result, deduped against tools that
  just reported the same job's terminal status).
- **Lifecycle**: bash blocks up to `wait` seconds (default 120) then the
  command *continues* as a background job; `background:true` detaches after a
  500ms fail-fast grace; optional `timeout` is a hard kill ceiling that stays
  armed past backgrounding. Finished job dirs are pruned after 48h
  (`jobTTL`); running jobs never are.

### Turn-start injection

The job table is also injected as turn-start context (`core.SourceJobs`,
ADR-0015 vocabulary): `codingspec.JobsContext(root)` renders running jobs plus
jobs finished in the last 10 minutes, and `pi.jobsInjector` wires it through
`engine.WithContextInjector` — which now accumulates injectors (memory, jobs,
future retrieval) instead of holding one.

Two disciplines keep the injection cheap:

- **Duration-free rendering.** The injected table omits runtimes, so its text
  changes only on real state transitions, keeping the fenced context block
  prefix-cache stable across turns.
- **Only append when changed** (the dedup ADR-0015 anticipated). A session is
  re-injected only when its rendered table differs from what it was last
  shown; the empty table (`NoJobsContext`) is injected only to *supersede* a
  previous table, so job-free sessions never carry the block at all.

## Consequences

- Jobs survive arbos crashes, restarts, and interleaved processes; the model
  re-grounds on them each turn without spending a tool call.
- The model never loses work to a default timeout; runaway commands are
  visible in `jobs` and killable by pgid.
- Exit codes for jobs that outlive their owner are exact (wrapper-written),
  not inferred. The one lossy state is honest: killed jobs report "no exit
  recorded".
- Injector dedup state is per-process: after a restart, a session whose table
  emptied while arbos was down keeps its stale segment until the table next
  changes. Accepted — jobs persist 48h, so the table is rarely empty right
  after a restart that mattered.
- `$TMPDIR` placement means a host reboot clears the table; pid-liveness
  derivation degrades gracefully (no exit file + dead pid → killed).

## Alternatives rejected

- *Pipe output through the agent* (Cursor's shape): loses output on agent
  death and makes streaming a supervisor concern. Rejected — the child
  journaling itself makes crash-safety free.
- *Job table in SQLite*: a schema, a migration, and a write path for state the
  filesystem already represents; and it would couple `codingspec` (standalone
  by design, see the schema-generator split) to the store. Rejected — derive,
  don't record.
- *Engine-level notification channel for job completion*: new kernel
  vocabulary for what tool-result notices plus turn-start injection already
  cover. Rejected for now; revisit if sleep/wake scheduling lands.
