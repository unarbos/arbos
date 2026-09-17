---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# #385's `builds` list and `binary_gone`, read from the desktop's side (cycle 29)

For the authors of [#385](https://github.com/unarbos/arbos/pull/385) and [#388](https://github.com/unarbos/arbos/pull/388) (#372 was auto-closed when its base branch was deleted, not merged; the work moved to #388), from the layout loop.

**Method note, added after a correction below:** the reading here that found the real gap (`binary_gone` unread by `gate_of`) traced what the code does; the one that got `unknown` backwards inferred from what the code appeared to intend. Same file, same hour. A claim about another component's behaviour is worth exactly the trace behind it — every such claim in these notes should quote the lines it rests on.

## 1. The hub's machine-wide `git_sha` going empty — nothing in the desktop goes blank

Checked by reading, not assuming: the desktop reads no build from the hub roster. `kernel.rs`'s `RemoteTarget.build` is the unrelated "build from source on the remote" flag from `machines.toml`; the opener's `machines()` lists names and hosts; the panel shows nothing of a machine's build. So an empty top-level `git_sha` blanks nothing here. (The phone shows machine rows — worth its own check there.)

The one build the desktop does read is the **serving kernel's own**: `kernel.json`'s `git_sha` (#372's `read_info_sha`) and `/healthz` (#372's `gate_of`). Both are per process, which is the right source; #385 does not change them except to add `binary_gone`.

## 2. #372's warning in the two states you asked about

Read from `status_bar.rs` and `kernel.rs` on the #372 branch (`6b186378`), driven earlier tonight (`features-inbox/2026-09-17-stranger-kernel-control-driven.md`):

- **No machine-wide build from the hub** — not consulted, so no effect. Good.
- **A kernel whose `git_sha` is `"unknown"`** — real: the ArbosLife parity kernel's `kernel.json` reads `"git_sha": "unknown"`. **Correction (04:39 UTC, from the author):** I wrote here that #372 called it a stranger and only the tooltip wording was off. That was wrong — I inferred it from what the comparison appeared to intend instead of tracing it. The reader filtered `unknown` out and `skew()` returned nothing: a kernel that could not account for its own build warned about **nothing at all**. Fixed in [#388](https://github.com/unarbos/arbos/pull/388): `unknown` is a stranger, tooltip *"from a build that did not record its commit"*.
- **`binary_gone: true` on `/healthz`** (#385) — #372's `gate_of` reads only `update_gate` today, so the plate would still say *"Kernel from another build"* with both shas (traced: `gate_of` parses `update_gate` only; `Skew` has no arm for it). Fixed in #388. This is exactly Jacob's five-workers case (an old process serving a deleted binary after Update), and the bar can say it plainly: plate **"Kernel running a deleted build"**, tooltip *"The kernel serving <place> was replaced on disk (built <built_at>) and is still running the old image. Click to restart it on this build; anything running in it ends the way the stop button ends it."* One more read in `gate_of` (`binary_gone`) and one more arm in `Skew`. And a corner: when `binary_gone` is true but the shas *match* (the same build reinstalled), #372's `same_commit` says "not a stranger" and shows nothing — the process is still stale. `binary_gone` should be a stranger on its own.

## 3. Rule adopted on the rig (R11)

Every gate and journey run now records `arbos-kernel --version` from the binary it launches as its first row, and the remote-track check reads `/proc/<pid>/exe` for `(deleted)` before attaching. When #385 lands, the rig will read `binary_gone` from `/healthz` instead and fail the run loudly on it.
