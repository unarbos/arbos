---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Kernel self-update — design

**For the features agent** (owns the kernel) and the **mesh worker** (runs the
hub and the live kernels on ArbosLife, and deploys this first).

The features agent answered the restart questions in
`internal/features-inbox/2026-09-16-kernel-self-update-restart-semantics-answers.md`;
everything settled there is folded in below rather than left open. The
sections marked *changed* or *corrected* say so on purpose — an earlier copy
of this document said the opposite, and two workers read it.

**Shipped so far:** [#308](https://github.com/unarbos/arbos/pull/308) (the feed
carries a `kernel` component; CI publishes a kernel payload for Linux and
macOS), [#318](https://github.com/unarbos/arbos/pull/318) (`arbos-kernel
update`). [#307](https://github.com/unarbos/arbos/pull/307) shipped the wire
fields and `idle::update_verdict`. **Left:** slice 4, the unattended part, and
the desktop skew indicator.

## The problem

Jacob's `arboslife/subnet120` ran days-old code and answered "hello" with a
directory listing. Two separate failures:

1. **Nothing told anyone.** `hello` carried only a semver that had not moved
   off `0.2.0` in weeks, so even shown it could not tell this morning's kernel
   from last week's.
2. **Nothing updated it.** It is hub-attached, so nobody ssh's in and the #229
   path never runs; nobody attaches either.

## Decisions, and why

### The kernel polls the feed

Not the hub pushing (a relay with no control plane — a new protocol *and* a
new trust path, and it would make the hub a far more valuable thing to
compromise), and not the client pushing on attach (that is #229, which already
exists and already misses this machine). Polling is the only option that
covers a kernel nobody ever contacts.

### The commit decides, not the version

`0.2.0` has not moved in weeks. `kernel::behind` compares the commit, using the
rule the ssh path has used since #229: older version, or the same version from
a different commit, is behind; newer is left alone.

Note `build` now means two things and they are never compared: in the feed,
`Release.build` is a commit count that orders releases; on the wire, `built_at`
is a UTC timestamp for showing a person how stale a machine is. #307 took the
rename, so the trap is gone.

### The moment: `idle::update_verdict`, ~10 s horizon

Corrected twice, and both corrections **widened** what may be updated. Written
out because the first version of this document said the opposite of each.

**What holds the gate:**

- **A turn in flight.** Including a pending approval — `hooks.approve()` blocks
  the tool call *inside* the running turn, so the agent never leaves
  `hooks.running` and `verdict` already says `Busy`.
- **A remote child mid-turn.** The swap closes its link, and it would report to
  a kernel that is no longer listening for it.
- A subscription run in flight.

**What does not:**

- **A parked question to the human.** It is a file, `clear_approves` does not
  touch it, and the answer arrives as an inbox file that opens a new turn.
  `Waiting` maps to `Idle`.

  **This is true from [#342](https://github.com/unarbos/arbos/pull/342) onward
  and was not true before it**, so it is a dependency and not a fact about the
  design. The file always survived, but a client attaching to the restarted
  kernel was never re-offered the question: the desktop rebuilt a card from the
  transcript line, and the phone would have shown a question with nothing to
  tap. Attach now re-offers pending asks as `ask` frames.

  It matters here more than it looks. `execv` keeps the pid, but it replaces
  the program image and closes the listening socket, so **every attached client
  reconnects and attaches fresh** — the re-attach path is squarely inside this
  feature's blast radius, and that is precisely where the gap was. Slice 4
  would have shipped a kernel that updated itself between turns and left
  somebody holding an unanswerable question.
- **A detached job** — *changed by [#321](https://github.com/unarbos/arbos/pull/321),
  and a consequence of `execv` rather than a separate decision.* The pid does
  not change, so a job's leash — which watches the kernel's pid — sees nothing
  happen. Jobs survive the swap, and the earlier rule would have held an update
  behind a background server for up to the 24-hour ceiling for no gain.

  **True from [#353](https://github.com/unarbos/arbos/pull/353) onward and
  false before it.** The reasoning in #321 had the boot reap taking only jobs
  orphaned to pid 1; driving it showed `reap_leftovers` killing *every* running
  job without a `keep` file, parent or not. So the new image would have ended
  exactly the jobs #321 decided were safe to update around — the first
  unattended update would have killed a background server and reported success.
  #353 inherits a job whose leash is parented to this very process instead of
  reaping it.

`keep` remains not the updater's to set.

**Horizon ~10 s**, the expected downtime, not `--until-idle`'s hour — with an
hour, a place holding an hourly timer is never idle and never updates.

Two further refusals: another process holds the place lock, and a
`kernel.json` naming a live pid that is not us. Both mean a second kernel is
serving.

### Restart: `execv`, one code path

**Changed twice on 2026-09-16**, each time towards something simpler.

The first version had two modes: exit under a supervisor, spawn-and-exit
without one. Then the live machines showed that `subnet120`'s parent is
`init` — the desktop spawns a remote kernel detached — so **nothing would
bring it back**, and a kernel that exits there is gone. That is the machine
this feature exists for.

`execv` is better than spawning for the unsupervised case:

| | spawn + exit | `execv` |
| --- | --- | --- |
| the new binary will not start | the kernel is **gone** | `execv` returns and the old image **keeps serving** |
| the place lock | two processes briefly want it → a ~30 s retry | same pid throughout; nothing else ever wants it |
| pid, and anything watching it | changes | unchanged |
| detached jobs | the leash kills them | **they survive** — the leash watches a pid that has not changed |

And then it turned out to be better for the supervised case too, so
**`ARBOS_SUPERVISED` is gone and there is one code path.** A supervisor sees
its process continue and has nothing to do; an unsupervised kernel keeps
serving. Two modes existed only because exiting was on the table.

**Exec the path, not `/proc/self/exe`.** After the swap Linux reports the image
as `<path> (deleted)` — exactly what was seen on `subnet120` on 15 September.
Capture the path *before* the swap and exec it by name, or the kernel re-execs
the build it was trying to replace, succeeds, and comes back identical with no
error anywhere.

Still required, unchanged: **same argv and environment** (`--leash`, `--hub`,
`--project`, `--bind` must survive), and **do not call `remote::stop_all`** on
the update exit — remote children are leashed and `remote::restore` re-attaches
them, so stopping them turns a restart into a teardown.

### The probe is load-bearing, because `execv` will not tell you

`execv` reports an error only when **the exec itself** fails — a bad
architecture, a missing interpreter, a file that is not executable. A binary
that starts and then dies while booting returns nothing, because by then the
old image is gone. There is no "it came up, so keep it" moment afterwards.

So all the confidence has to be bought *before* the old binary is moved aside,
and [#318](https://github.com/unarbos/arbos/pull/318) buys as much as it can:

- **`Probe::Version`** — the staged binary runs and reports the version the
  feed promised. Proves it links, reaches `main`, and is for this machine.
- **`Probe::Place`** — and it reads a real place *no worse than the binary it
  replaces*, via `arbos-kernel check <place>`, which walks the store with the
  parsers `serve` boots on. Compared with the old binary rather than required
  to be clean: a place with pre-existing errors is not the new build's fault,
  and refusing on it would make updates impossible on exactly the machines
  that need one.

And for the failure no probe can see, the binary that was replaced is kept as
`<bin>.previous`. Not hidden, unlike the staging and backup names — somebody
looking at a kernel that will not start should find the one that did sitting
next to it. It turns "the new kernel dies at boot on a box nobody watches"
from unrecoverable into one `mv`.

### Replacing the binary is not updating the kernel

The running process keeps its image until it restarts. Therefore **any skew
display must read the running process's commit**, never the file:

- `hello` and the hub roster already do — the fields are compiled into the
  process that sends them.
- `kernel.json` carries the `version` and `git_sha` of the process that wrote
  it, which is the authority for a place on this machine.
- `arbos-kernel update` reports both and marks a running kernel older than the
  binary beside it (#318). Reading the file alone would report a machine as
  current while it served old code — which is the `subnet120` failure exactly.

### Saying no

A binary under `target/debug` or `target/release` is somebody's working copy; a
`--pin` holds; a newer build is left alone; a channel with no kernel for this
platform is an answer, not an error. Only the last is a non-zero exit.

### Channel

`dev` when the machine is registered with a hub, `stable` when it is not —
Jacob's ruling. A machine on the hub is part of the mesh and is meant to track
`main`. Automatic is on by default, with the refusals above as the safety.

## The app's update has the same problem, and it has already done harm

*Added 2026-09-17 after an incident on Jacob's Mac. Evidence: `qa-results`
branch, `inbox/hung-five-workers/`, commit `8b8cf9e5`.*

At 22:20 the in-app updater swapped the bundle and relaunched. **Its kernels
kept running from the deleted old binary** — one executing
`/Applications/.Arbos.app.arbos-old/…/arbos-kernel`, 223 commits behind — and
the new app attached to them anyway. It then sent frames that kernel has never
heard of: `frame_rejected: unknown frame type "feedback"`. Jacob's feedback
sheet showed three empty rows, and five workers finished their jobs and never
reported.

This is the kernel self-update problem arriving from the other side, and the
design should carry both.

### Why the stop did not stop them

`desktop/src/update.rs` does send `SIGTERM` before the swap. The code is right
for the places it is *given*, and that is the bug: it is given
**`workspace.projects` — the tabs open in that window at the moment of the
click.** A kernel outside that list is never asked to stop:

- a place whose tab was closed, or which was never opened in this window;
- a kernel another window, the CLI, or a worker started;
- a kernel that outlived an earlier update and has been accumulating skew since
  — which the 223 commits suggest is what happened, the path naming whichever
  swap first orphaned it.

And two further holes even for places it does know about: a kernel that does
not die inside `KERNEL_STOP_WAIT` is left running *by design*, and
`Swap::commit` then deletes the backup directory out from under it, which is
how a live process ends up executing a path that no longer exists.

### The fix is two-layered, because stopping alone cannot be trusted

Stopping is best-effort by nature — the app cannot enumerate every kernel on
the machine, and a wedged kernel must not block an update. So it needs a
second layer that does not depend on the first having worked.

1. **Stop more of them.** Every place this app has a kernel for, not only the
   open tabs — the app knows its recents and its per-place runtime files.
2. **Never attach silently to a kernel that is not this bundle's.** At attach,
   compare the running kernel's commit — `kernel.json` carries the `git_sha` of
   the process that wrote it, and `hello` carries it too since #307 — with the
   kernel this bundle ships. On a mismatch:
   - **ask its gate.** `/healthz` carries `update_gate verdict=… reason=…`
     since [#353](https://github.com/unarbos/arbos/pull/353). Idle means stop
     it and let the app respawn from the bundle.
   - **busy means say so and do not proceed silently.** A turn in flight, a
     parked question or a detached job is exactly what must not be thrown away
     — the same gate, for the same reason, from the app's side.

The second layer is the one that would have prevented this incident, because
it holds however the first one fails.

### What the skew display would have done

Nothing about this was subtle: 223 commits behind, on the bar, next to the
version. The display was designed to make exactly this visible and has not
been built. This incident moves slice 1b from "worth doing" to the thing
blocking a safe app update.

## Slice 4 must be driven, not reasoned

Three claims about what survives a restart were read out of the code and
written into this document. **All three needed correcting once somebody ran
them:**

| claim | read as | driven |
| --- | --- | --- |
| a parked ask survives | fine across a restart | the file survived; the question was never re-offered to a reconnecting client ([#342](https://github.com/unarbos/arbos/pull/342)) |
| a cut approval does not run | true, and reported honestly | true, but the record told the model it "may have completed in part or in full" ([#342](https://github.com/unarbos/arbos/pull/342)) |
| a detached job survives `execv` | the boot reap takes only pid-1 orphans | `reap_leftovers` killed every job without `keep`, parent or not ([#353](https://github.com/unarbos/arbos/pull/353)) |

Every one was a correct reading of the code and wrong about the behaviour. The
last would have killed a background server on the first unattended update and
reported success.

This design's gate — what may be updated and what may not — is built entirely
from claims of that kind. So slice 4 does not ship on reasoning.

### What has to be watched happening, on a real kernel

Two of these stopped being inferences when #353 added the log lines, which is
the difference between checking and hoping:

1. **A parked ask across an update**, with a desktop and a phone attached. The
   question is re-offered to both and answering it still works.
2. **A detached job across the swap.** *Readable now:* boot logs
   `job_inherited` and `jobs_alive count=… <id>:pid=…`, so the run compares ids
   and pids either side of the swap rather than inferring survival from the
   job still looking alive.
3. **A remote child mid-turn refuses.** *Readable now:* the gate logs
   `update_gate verdict=… reason=…` and `/healthz` carries it, so a refusal can
   be read from outside before any swap is attempted.
4. **A subscription run in flight refuses.** The one gate condition nobody has
   driven from either side. It is in `verdict` because `subs::busy()` says so,
   which is the same kind of claim as the three above. `update_gate` on
   `/healthz` makes it checkable the same way.
5. **A binary that passes the probe and then dies at boot.** `<bin>.previous`
   is there and moving it back recovers the machine.
6. **`execv` on a box with no supervisor** — `subnet120` is the case — with the
   path captured before the swap, confirming it comes back as the *new* build
   and not the deleted inode.

Any of the six that cannot be driven is a reason to hold the slice, not a
reason to write a more confident sentence about it.

## Slices

| # | slice | state |
| --- | --- | --- |
| 1 | `git_sha` + `built_at` on `hello`, `/healthz`, `Register`, `MachineInfo` | **done**, #307 |
| 1b | the desktop showing kernel skew in the bar and on the roster | **left** |
| 2 | `component` on `Download`; kernel payloads for Linux and macOS | **done**, #308 |
| 3 | `arbos-kernel update`, run by a person | **done**, #318 |
| 4 | poll, idle gate, config, pin, `channel = "off"`, `execv` restart | **left** |
| 5 | #229 installs the feed's signed artifact rather than the client's local binary | **left** |

## Open with the mesh worker

`internal/mesh-inbox-kernel-self-update-deploy.md` asked three questions; the
`subnet120` finding answers the first and changes it: **not every kernel has a
supervisor**, so slice 4 cannot assume a restart loop. Still open: whether
anything pins a kernel binary in a way a rename breaks, and whether every green
merge restarting every idle kernel would be disruptive on ArbosLife.
