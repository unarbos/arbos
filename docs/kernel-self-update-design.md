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
- **A detached job** — *changed 2026-09-16 by [#321](https://github.com/unarbos/arbos/pull/321),
  and a consequence of `execv` rather than a separate decision.* The pid does
  not change, so a job's leash — which watches the kernel's pid — sees nothing
  happen, and the boot reap only takes jobs orphaned to pid 1. Jobs survive the
  swap. The earlier rule would have held an update behind a background server
  for up to the 24-hour ceiling, for no gain.

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

## Slice 4 must be driven, not reasoned

Three claims about what survives a restart were read out of the code this
morning and written into this document. Two of them were wrong when somebody
actually ran them ([#342](https://github.com/unarbos/arbos/pull/342)): a parked
ask survived the file but was never re-offered to a reconnecting client, and a
cut approval was described to the model as "may have completed in part or in
full" when the call had never run at all. The third held.

Both were correct readings of the code and both were wrong about the behaviour,
which is the point. This design's gate — what may be updated and what may not —
rests entirely on claims of that kind, so slice 4 does not ship on reasoning.

What has to be **watched happening**, on a real kernel, before it runs
unattended:

1. An agent parked on a question, a desktop and a phone attached, an update in
   between. The question is re-offered to both and answering it still works.
2. A detached job running across the swap. It is still running afterwards, and
   its output did not stop — the claim that the leash sees an unchanged pid is
   the reason jobs no longer hold the gate, and it is untested from this side.
3. A remote child mid-turn. The gate refuses, and does not merely appear to.
4. A new binary that passes the probe and then dies at boot. `<bin>.previous`
   is there and moving it back recovers the machine.
5. `execv` on a box with no supervisor — `subnet120` is the case — with the
   path captured before the swap, confirming it comes back as the *new* build
   and not the deleted inode.

Any of the five that cannot be driven is a reason to hold the slice, not a
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
