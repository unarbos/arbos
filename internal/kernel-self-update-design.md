---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Kernel self-update — design

Replaces the copy I first wrote at `docs/kernel-self-update-design.md`, which
is no longer in the store. My standing instruction is that my working notes
live in `internal/`, so it belongs here. The features agent read the earlier
one and answered it —
`internal/features-inbox/2026-09-16-kernel-self-update-restart-semantics-answers.md`
— and everything settled there is folded in below rather than left as an open
question.

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

Corrected by the features agent; my first version guarded the wrong state.

- **A pending approval is already `Busy`** — the tool call blocks inside the
  running turn, so the agent never leaves `hooks.running`.
- **A parked question to the human is *not* a reason to hold.** It is a file,
  `clear_approves` does not touch it, and the answer arrives as an inbox file
  that opens a new turn. `Waiting` maps to `Idle`. This removes a reason to sit
  stale, which is the point of the feature.
- **Running jobs and remote children mid-turn are `Busy`** — `verdict` cannot
  see them, and both die with the kernel whatever a `keep` file says. `keep` is
  never the updater's to set.
- **Horizon ~10 s**, the expected downtime, not `--until-idle`'s hour — with an
  hour, a place holding an hourly timer is never idle and never updates.

Two further refusals: another process holds the place lock, and a `kernel.json`
naming a live pid that is not us. Both mean a second kernel is serving.

### Restart: `execv` where nothing supervises

**Changed 2026-09-16 after looking at the live machines**, and this is the most
important correction in the document.

`subnet120`'s parent is `init` — the desktop spawns a remote kernel detached —
so **nothing would bring it back**. A kernel that exits there is gone, and it is
the machine the whole feature exists for.

| | spawn + exit | `execv` |
| --- | --- | --- |
| the new binary will not start | the kernel is **gone** | `execv` returns an error and the old image **keeps serving** |
| the place lock | two processes briefly want it, so the child must retry ~30 s | same pid throughout; nothing else ever wants it |
| pid, and anything watching it | changes | unchanged |

So: `ARBOS_SUPERVISED=1` → exit with a distinct code and let the loop restart
it. Otherwise → `execv`. The 30-second lock retry the features agent asked for
was a consequence of spawning, and spawning has been dropped.

**Exec the path, not `/proc/self/exe`.** After the swap Linux reports the image
as `<path> (deleted)` — exactly what was seen on `subnet120` on 15 September.
Capture the path *before* the swap and exec it by name, or the kernel re-execs
the build it was trying to replace.

**A kernel that cannot re-exec refuses out loud**, where skew is already shown
— the log, `hello`, the roster. A silent refusal is the same failure as the one
being fixed.

Still required, unchanged: **same argv and environment** (`--leash`, `--hub`,
`--project`, `--bind` must survive), and **do not call `remote::stop_all`** on
the update exit — remote children are leashed and `remote::restore` re-attaches
them, so stopping them turns a restart into a teardown.

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
