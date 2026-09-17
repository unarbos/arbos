---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

> **Rewritten 08:10 UTC.** The first version of this note was written between 06:41 and 06:52 on 2026-09-17 and was lost when the QA loop's test agent ran `rm -rf` over the store (`docs/store-fault-report-2026-09-17.md`); the restore carried only the 06:41 mirror. This version is rebuilt from the cycle-30 log entry in `internal/symmetry-prompts.md` (which survived, mtime 07:18) and from a fresh enumeration on ArbosLife at 08:07 UTC. Nothing in it is inferred from memory alone.

# The `~/.cargo/bin/arbos-kernel` replacement on ArbosLife at 05:23 — it was mine

**Answer: yes.** My loop replaced `/home/const/.cargo/bin/arbos-kernel` on ArbosLife at about 05:23 UTC on 2026-09-17, and my parity kernel restarted onto it four minutes later.

## What the record says

The cycle-30 entry in `internal/symmetry-prompts.md` (written at the time, before the store loss, still in place) records the act in my own words:

> **ArbosLife reinstall through the app's own path — done:** with `machines.toml` restored (`build = true`), opening `arboslife:…/parity-proj--reply-with-hostname` had the app `cargo install` `main`'s kernel there (`~/.cargo/bin/arbos-kernel`, 05:23) and, once the old process was stopped, start it: pid 917740, `arbos-kernel 0.2.0 d73a25aea876`.

So: a 258 MB `cargo install` source build of `d73a25ae`, written by the app's remote-install path to the default `REMOTE_BIN`, which was `$HOME/.cargo/bin/arbos-kernel` — the shared path on Jacob's `PATH`. That is the file his `subnet120` project was running; replacing it left that process on a deleted inode.

## Why the app wrote to the shared path

Two things together:

1. `desktop/src/kernel.rs` had `REMOTE_BIN = "$HOME/.cargo/bin/arbos-kernel"` as the fallback when a machine's `machines.toml` row gives no `kernel` path.
2. My drive scripts launched the app with a scratch `XDG_CONFIG_HOME` and did not copy `~/.config/arbos/machines.toml` into it, so the row with my private path was not read, and the fallback was used.

## What changed so it cannot recur

- [#404](https://github.com/unarbos/arbos/pull/404): `REMOTE_BIN` is now `$HOME/.arbos-remote/bin/arbos-kernel` — a directory the app owns, not the shared `~/.cargo/bin`. This is the change that moves the app off the shared path.
- Every drive script under `/tmp/c23/` that launches the app with a scratch config home copies `machines.toml` into it first (`worker32_drive.py` line 26 is the pattern).
- My parity kernel on ArbosLife runs from a binary I own (below).

## Enumeration, 08:07 UTC 2026-09-17 (read from the machine, not the roster)

| file | size | mtime | `--version` |
| --- | --- | --- | --- |
| `/home/const/.cargo/bin/arbos-kernel` (shared) | 26 541 648 | 06:25:17 | `0.2.0 30eef166a191` |
| `/home/const/arbos-remote/bin/arbos-kernel` (mine) | 26 541 648 | 06:47:07 | `0.2.0 30eef166a191` |

Processes:

| pid | started | binary | project |
| --- | --- | --- | --- |
| 1427348 | 06:41:42 | `/home/const/.cargo/bin/arbos-kernel` | `/home/const/subnet120` (Jacob's, restarted after the incident) |
| 1449445 | 06:44:58 | `/home/const/arbos-qa/bin/arbos-kernel` | QA's cycle-11 demo |
| 1464964 | 06:47:08 | `bin/arbos-kernel` in `/home/const/arbos-remote` | `parity-proj--reply-with-hostname` (mine) |

My kernel is on my own file; a replacement of the shared file cannot leave it stale, and a replacement of mine cannot touch Jacob's or QA's. The hub's own processes (`arbos-hub/bin/…`, since 04:46) and the phone loop's (`arbos-hub/phone/bin/…`) were never on the shared path.

## The `subnet120` line

Jacob's `subnet120` kernel (pid 1427348) was restarted at 06:41:42 onto the current shared binary (`30eef166a191`, written 06:25 — not by me; my last write to that path was 05:23). It is no longer running a deleted binary.
