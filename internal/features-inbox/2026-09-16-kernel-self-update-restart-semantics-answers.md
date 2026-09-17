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
3. **`verdict` does not see remote children.** A remote child mid-turn
   reports to a link the swap closes. (It does not see detached jobs
   either — but with `execv` that no longer matters; see below.)

So: **`idle::update_verdict(hooks, horizon_ms)`** (#307, corrected in
[#321](https://github.com/unarbos/arbos/pull/321)) = `verdict` with
`Waiting → Idle`, plus `Busy` for any remote child mid-turn
(`remotes.is_running`). `--until-idle` is unchanged. Pass `horizon_ms` =
your expected downtime (about 10 s), not `--until-idle`'s hour, or a
place with an hourly timer never updates.

Your 24 h ceiling: when it fires, nothing on this list is knowingly
lost except a remote child's report, which arrives late as a `say`.

## 1. Who restarts — `execv`, and that settles it (revised 14:10 UTC)

You chose `execv` over spawn-and-exit. That is the better answer, and it
makes most of my earlier §1 obsolete:

- **No lock retry.** The pid does not change. Rust's `File` is
  `O_CLOEXEC`, so the flock drops at exec and the new image takes it
  again with nothing contending. Forget the 30 s retry I asked for.
- **No `ARBOS_SUPERVISED` split.** A supervisor sees one process
  continue; an unsupervised kernel keeps serving. Same code path
  everywhere.
- **If the new binary cannot be exec'd, `execv` returns and the old image
  keeps serving** — the failure mode that matters on `subnet120`, where
  an exit would have left no kernel. One caveat to keep in view: `execv`
  fails only at exec time (format, permissions). A binary that starts and
  then dies at boot (a config it cannot read, a panic) is not caught by
  that return; the pre-swap `--version`/health probe in `arbos-update`
  (#308) is what covers it, so keep that probe in front of the exec.
- **Re-exec with the same argv and env** still applies (`--leash`,
  `--hub`, `--project`, `--bind`, `LEASH_ENV`): `execv(current_exe(),
  args_os())`.
- **Do not stop remote children on the swap.** Same as before: skip
  `remote::stop_all`; `remote::restore` re-attaches from `remotes.json`.
- `kernel.json` keeps its pid; the port comes back the same with the same
  `--bind`. Attached sockets close at exec (`O_CLOEXEC`); clients
  reconnect and replay the tail, as after any restart.

## 2. Jobs — they survive an `execv`; never mark `keep`

With `execv` the job question answers itself. A detached job's leash
watches the kernel's pid (`K=$PPID` in `jobs.rs`), which the swap keeps,
and the boot reap takes only jobs whose parent is pid 1. So jobs run on
across the swap, owned by the same pid: the new image lists them from their
folders as before. `update_verdict` no longer counts a running job as
`Busy` (#321). `keep` stays what it is — the user's word that a job may
outlive kernels — and the updater does not touch it.

## Driven, not reasoned (added 18:20 UTC, [#342](https://github.com/unarbos/arbos/pull/342))

The three claims above were tested with SIGKILL in the parked state and a
new kernel on the same place. Two needed a fix, which is why I say so
here rather than let the note stand on reading alone:

- **Ask survives:** true for the file and the answer path. But a client
  attaching to the new kernel was not offered the question as a card —
  only the transcript line was replayed. Attach now re-offers pending
  asks as `ask` frames with their ids (`asks_replayed` in the log).
- **Approval does not survive:** true, and nothing runs without the
  click. But the cut record said the call "may have completed"; it now
  says "not run … waited for the user's allow/deny … Nothing changed."
- **Parent in `spawn wait`:** held as claimed; pinned.

Nothing here changes the gate (`update_verdict`): it still waits on a
running turn, which is what an open approval is, and lets an ask through.

## Item 2 driven: the boot reap was taking the jobs (added 21:20 UTC, [#353](https://github.com/unarbos/arbos/pull/353))

§2 above said the boot reap "takes only jobs whose parent is pid 1".
That was the comment's intent, not the code: `reap_leftovers` killed every
running job without a `keep` file, whatever its parent. Across an `execv`
the new image would have ended every detached job — the very jobs the
gate stopped waiting for. Fixed in #353: a job whose leash's parent is
this process itself is inherited, not reaped, and boot logs
`job_inherited` and `jobs_alive count=N <id>:pid=<pid>`. Driven by a
helper that starts a job the kernel's way and then execs into
`arbos-kernel serve`: same pid, job still writing under the new image,
and ended when that pid dies.

Item 3 is readable the same way: `update_gate verdict=busy
reason="<child>: a turn runs on <machine>"` in the log whenever the gate is
asked, and `GET /healthz` carries `update_gate: {verdict, reason}` so you
can read the refusal from outside before the swap.

## Item 6 driven: a subscription run in flight (added 2026-09-17 00:35 UTC, [#364](https://github.com/unarbos/arbos/pull/364))

`subs::busy()` holds: `run_job` awaits the command to its end and the
in-flight mark clears only after the outcome lands. The gate now names
the run (`subscription runs in flight: root#2 shell \`…\` (4s)`) on
`/healthz` and in the log. The ceiling case — you stop waiting and swap
mid-run — is said rather than lost: the run's job carries a
`subscription` marker, and the next boot logs `subscription_run_cut`
and writes it on the row's `last`; `next_due` stands. All six items are
now readable from the kernel: 1 (#342), 2 (#353), 3 and 6 (`update_gate`
reason), 4 and 5 are yours.

## What a restart preserves (all files) and what it cannot

Preserved, no work needed: asks and their answers; inbox files; subscriptions
(a missed firing catches up once at boot, `catch_up = once`); notifications
and `seen`; attachments (`attachments/` under the agent); `remotes.json` and
the remote kernels behind it (leashed ≥ 10 min, re-attached by `restore`);
notes, plan, page; `blocked-models.json`; the hub registration (re-registers
on boot; clients on hub channels see "kernel went away" and reconnect —
that is the existing #275 path).

Preserved by `execv` specifically: detached jobs (their leash watches a
pid that does not change), the place lock (same pid), `kernel.json`.

Cannot be preserved, and the gate refuses while they exist: a turn in
flight (including a kickoff turn), a pending approval, a tool call
mid-batch, a remote child mid-turn, a subscription run in flight.

One more refusal for the updater: **`kernel.json` names a pid that is not
us** — a second kernel is serving this place; abort the update and log
it. (With `execv` the place lock is ours throughout, so there is no lock
case to add.)

## Reply on the frame fields

No objection; they are shipped as `git_sha` and `built_at`. The hub reports
what each machine *is* (version, commit, built-at) and does no ordering:
it cannot know what "current" is without fetching the feed, and that would
put a network dependency on the one box everything registers with. The
client, which already holds the feed for its own updates, decides what is
stale and how to order the roster. Show `built_at` on the roster, not the
sha.
