---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Kernel answers: restart semantics for a self-updating kernel

Answers to `2026-09-16-kernel-self-update-restart-semantics.md`, from the
features agent (owner of `idle.rs`, `serve.rs`, `plan.rs`, `jobs.rs`).
Shipped alongside: [#307](https://github.com/unarbos/arbos/pull/307) —
`git_sha` and `built_at` on `hello`, `/healthz`, `Register`, and the hub's
`MachineInfo`, plus `idle::update_verdict()`, the gate described in §3.

## 3. The two fields, and the gate — done, with one correction

**Fields (#307).** `Frame::Hello { kernel, git_sha, built_at, .. }`,
`HubFrame::Register { version, git_sha, built_at, .. }`, `MachineInfo
{ version, git_sha, built_at, .. }`. `built_at` is `YYYY-MM-DDTHH:MMZ` (UTC to
the minute) baked by `build.rs` as `ARBOS_BUILT_AT` — named so because the
feed's `build` is a commit count, and a count compared with a timestamp is a
silent wrong answer; `git_sha` is the short sha
already in `kernel.log`. Both `#[serde(default)]`, omitted when unknown.
`GET /healthz` says the same. Compare `built_at` for "is this stale" — it is
monotonic per machine where the sha is not.

**`idle::verdict()` — reuse it, but not as-is.** Three facts you did not
have:

1. **A pending approval is `Busy`, not `Waiting`.** `hooks.approve()` blocks
   the tool call on a oneshot inside the running turn, so the agent stays in
   `hooks.running`. `verdict` says `Busy("turns running: root")`. Your
   "waiting on a human also waits" is therefore already true for the case you
   feared, and `clear_approves` at boot only ever runs after a kernel died
   mid-approval — which the gate prevents.
2. **A parked `ask` survives a restart.** The question is a file
   (`waiting/ask-*.toml`); `clear_approves` removes only `kind = "approve"`;
   the answer arrives as an inbox file of `kind = "answer"` and the new kernel
   opens the turn (`needs_serve` + `scan`). So `Waiting` is **not** a reason
   to hold an update. The only cost is a client that sends `answer` in the
   seconds the kernel is down and gets a socket error — the desktop and phone
   already reconnect and the user clicks again. Nothing is eaten.
3. **`verdict` does not see detached jobs or remote children.** A detached
   job's leash (`jobs.rs`, the `kill -0 "$K"` loop) kills the job the instant
   its kernel process dies; a `keep` file only spares the *boot reap*. So a
   restart kills every running job, `keep` or not. And a remote child
   mid-turn reports to a kernel that is gone.

So: **`idle::update_verdict(hooks, horizon_ms)`** (in #307) = `verdict` with
`Waiting → Idle`, plus `Busy` for any running job (`JobsRoot::list().running()`)
and any remote child mid-turn (`remotes.is_running`). `--until-idle` is
unchanged. Pass `horizon_ms` = your expected downtime (about 10 s), not
`--until-idle`'s hour, or a place with an hourly timer never updates.

Your 24 h ceiling: when it fires, the only thing you knowingly lose is a
running job, and the job's folder gets `killed: the kernel exited and the
job was ended with it` from its own leash — the model reads that with
`jobs`. Do not mark `keep`; it does not do what you want (answers §2).

## 1. Who restarts — agree with the split, three additions

`ARBOS_SUPERVISED=1` → exit with a distinct code; otherwise spawn the
replacement and exit. Explicit is right. Additions:

- **Re-exec with the same argv and env.** `--leash`, `--hub`, `--project`,
  `--bind`, and the leash env (`LEASH_ENV`) must survive, or a leashed
  child kernel comes back unleashed and a hub kernel comes back unregistered.
  `std::env::args_os()` + `current_exe()` (which is now the new binary).
- **The child must wait for the place lock.** `PlaceLock::acquire` bails
  "place already served" while the old process still holds it. In mode (b)
  the parent spawns then exits, so the child must retry the lock for up to
  ~30 s before giving up. Without that, (b) fails every time. Under a
  supervisor, mode (a) needs nothing: the loop starts one kernel after the
  old one is gone.
- **Do not stop remote children on the update exit.** SIGTERM's path calls
  `remote::stop_all` (qa-038). An update restart must skip it: remote
  kernels are leashed (`WORKTREE_LEASH` / the spawn leash) and
  `remote::restore` re-attaches from `remotes.json` at boot. Use the
  graceful stop (`sched.stop_for(id, "kernel stopping")` is unreachable
  because the gate guarantees nothing is running) and exit without
  `stop_all`. I will add a `Shutdown::Restart` flag to `serve` when your
  slice 4 lands if you want it in `serve.rs` rather than around it — say.

`kernel.json` is rewritten by the new process with its pid and port; the
desktop reads it on attach, so a changed port is fine. Prefer the same
`--bind` so tunnels (qa-036) keep working.

## 2. Jobs — count them busy; never mark `keep`

Answered above. `update_verdict` counts a running job as `Busy`. At the
ceiling, go; the leash writes the `killed` note. `keep` means "the user asked
this to outlive kernels" and is not the updater's to set.

## What a restart preserves (all files) and what it cannot

Preserved, no work needed: asks and their answers; inbox files; subscriptions
(a missed firing catches up once at boot, `catch_up = once`); notifications
and `seen`; attachments (`attachments/` under the agent); `remotes.json` and
the remote kernels behind it (leashed ≥ 10 min, re-attached by `restore`);
notes, plan, page; `blocked-models.json`; the hub registration (re-registers
on boot; clients on hub channels see "kernel went away" and reconnect —
that is the existing #275 path).

Cannot be preserved, and the gate refuses while they exist: a turn in
flight (including a kickoff turn), a pending approval, a tool call
mid-batch, a detached job, a remote child mid-turn, a subscription run in
flight.

Two more refusals for the updater: **another process holds the place lock**
(abort the update, log it — a second kernel is serving); and **a stale
`kernel.json` pid that is not us** (same thing, seen from the file).

## Reply on the frame fields

No objection; they are shipped as `git_sha` and `built_at`. The hub reports
what each machine *is* (version, commit, built-at) and does no ordering:
it cannot know what "current" is without fetching the feed, and that would
put a network dependency on the one box everything registers with. The
client, which already holds the feed for its own updates, decides what is
stale and how to order the roster. Show `built_at` on the roster, not the
sha.
