# Hung five workers — read-only capture from Jacob's Mac

Captured 2026-09-17T01:24Z (22:24 local, UTC-3), about four minutes after the
prompt, with nothing restarted. Place: `~/Code2`. Prompt at 22:20:42 local:
"spawn 5 sub agents that all return 1 number at random".

All text here went through `sed -E 's/[A-Za-z0-9_-]{32,}/[REDACTED]/g'`, so any
32+ character run (hashes, long ids) reads `[REDACTED]`. Nothing else edited.

## What is here

- `agents-tree.txt` — `find -ls` of `.arbos/agents/` (pages/ omitted).
- `agents/random-number-{1..5}/` — each worker's `agent.md`, `status.toml`,
  the whole `transcript.jsonl` (one line each), `jobs/j1/{meta.json,out.log,exit}`,
  `turns/t0001/{cause.md,meta.toml}`, and the inbox listing.
- `agents/root/` — `agent.md`, `status.toml`, last 30 transcript lines
  (the prompt is the last two), turns t0007–t0009, jobs j9/j10, inbox and
  subscriptions listings.
- `kernel.json`, `runtime/kernel.json`, `runtime/lock`, `runtime/focus`.
- `runtime/kernel.log-from-22h18.jsonl` — the kernel log from 22:18 local
  onward (261 lines); `runtime/kernel.out.log` — the kernel's stdout/stderr.
- `notifications.jsonl`, `spend.toml`.
- `processes.txt` — live process snapshot: every arbos process (no env),
  children of the Code2 kernel, the five job pids, which binary each kernel is
  executing, the bundle on disk.

## What the evidence shows (observations, not a diagnosis)

1. The Code2 kernel is pid 1883, started 07:04 local today, `git 3940aac3da21`
   (Merge #293, commit 962 on main — 223 commits behind `5017ef45`, the
   1185 build the app itself is running). `lsof` shows it executing
   `/Applications/.Arbos.app.arbos-old/Contents/MacOS/arbos-kernel`, a path
   that no longer exists: the in-app updater swapped the bundle at 22:20 and
   relaunched the app (pid 88278, 22:20:01), but this kernel — and the
   `.arbos`, `Code/Agent` and `Bdisco` kernels — kept running from the deleted
   old binary. Only the `ArbosClaw` kernel, spawned after the update, runs the
   new one. The new app attached to the old Code2 kernel.
2. `kernel.out.log` ends with four `frame_rejected: unknown frame type
   "feedback"` — the 1185 app speaking to a #293-era kernel.
3. Root's turn (model `inception/mercury-2.5`) started 22:20:42 and spawned
   five workers; each worker's turn started 22:20:44–45 (model
   `anthropic/claude-fable-5.1`, `model: inherit` in `agent.md`). After those
   `turn_start`/`prompt_size` lines the kernel log records nothing for any
   of the six agents: no tool event, no `turn_complete`, no error.
4. Yet each worker did run its tool: `jobs/j1/meta.json` shows
   `python3 -c "import random; print(random.randint(0,100))"` started
   22:20:50, `exit` is `0`, `out.log` holds the number (worker 1: `26`).
   `status.toml` still says "Generating random number via python".
5. All five python pids are zombies (`Z`, parent 1883): exited, never
   reaped. The kernel has no other children and no outbound TCP connection,
   so no model call is in flight for any of them. Root's transcript ends at
   the user line (no assistant text), and its status is the derived
   "Starting worker random-number-5".
6. `/Applications/.Arbos.app.arbos-new` is an empty leftover directory from
   the 22:20 update.

Screenshot referenced by the coordinator lives outside this Mac and is not
included.
