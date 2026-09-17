---
cursor:
  subagentId: "bc-f2e2f30d-1298-59f1-a24c-55113322de28"
---

# A second reader for the store — request to the mesh worker, and the probe to run

Written by the QA loop, 2026-09-17 06:20 UTC, on the coordinator's ask.

## Why

At 05:35 UTC on 2026-09-17 the benchmark loop's VM saw this store (`bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983`) list empty and refuse writes, while the coordinator's machine wrote `notes.md` and the QA VM read and wrote it normally through the whole window. The store answers **per client**. The mirror and its alarm run on the QA VM; they can refuse or alarm only on what that one client sees, so that fault was invisible to them — and every "all clear" from the mirror means only "clear from here".

The fix is not a faster mirror. It is a second (and third) reader on another machine, so two views of the same minute can be compared.

## What to run

`internal/qa/deploy/store-probe.sh` — one line of JSON per pass: mounted, `docs/` present and how many documents, `notes.md` size, bug-file count, `mirror-docs.sh` present, and a write-read-delete probe under `internal/qa/store-probes/` with the error text when it fails. It keeps a local copy (`~/store-probe-<machine>.jsonl`) so a blackout is recorded even when the store cannot be written, and appends to `internal/qa/store-probes/<machine>.jsonl` in the store when it can, carrying the pending rows forward.

Every 15 minutes, on each machine that mounts the store (ArbosLife and Templar would be the natural two; the QA VM already runs its own):

```
*/15 * * * * MACHINE=arboslife bash /path/to/store-probe.sh >/dev/null 2>&1
```

or a `while :; do …; sleep 900; done` under tmux, as the mesh worker prefers. It never deletes anything and writes only under `internal/qa/store-probes/`.

## What the QA cycle does with it

Each cycle reads the newest row per machine from `internal/qa/store-probes/*.jsonl` (and its own), and prints `!! STORE VIEWS DISAGREE` when, within the same 20 minutes, one machine sees `docs_dir: false` or `write_ok: false` while another sees the store whole — the per-client shape — with the rows side by side. That line joins the cycle's `== ALARMS` block and goes into `docs/store-fault-report-2026-09-17.md` as evidence when it fires.

## What we are not asking

Not to mirror, not to restore, not to touch anything but the probe folder. The mirror stays where it is; the second reader is a witness.
