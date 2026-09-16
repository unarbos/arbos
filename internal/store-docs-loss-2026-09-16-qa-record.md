---
cursor:
  subagentId: "bc-f2e2f30d-1298-59f1-a24c-55113322de28"
---
# QA worker's record for the `docs/` loss of 2026-09-16

Companion to `internal/store-docs-loss-2026-09-16.md` (the store-recovery worker's report; not edited here, it is theirs).

## What this worker saw

- **07:22:37 UTC** — `ls -la` of the store root listed `artifacts/ docs/ internal/ media/ notes.md`.
- **07:43:28 UTC** — this worker's last write into `docs/qa-loop-design.md` (a `StrReplace`, batch `43d8569d` section) succeeded; the file was 45,197 bytes by the recovery worker's count.
- **09:10 UTC** — a `StrReplace` on the same file failed with `FileNotFoundError`; `ls` of the store root showed `internal/ media/ notes.md` only; `stat docs` → no such file. `notes.md` still carried 17 links into `docs/`.
- Between those moments the worker ran only scenario code under `/tmp` and `~/arbos-qa`, and wrote under `internal/qa/` (`batch_scenarios.py`, `run.py`, `bugs/qa-039-*.md`, rollouts). No command named `docs/` or `artifacts/`.

## What was rebuilt

`docs/qa-loop-design.md`: rebuilt from `internal/qa/bugs/`, the scenario docstrings, `kickoff-history.jsonl`, the PRs and this worker's history; 27,442 bytes against the original's 45,197. The recovered 5,608-byte fragment (Phases, File-based agent model, Nightly SWE-bench) is kept and marked *evidenced*; everything else is marked *reconstructed* per section. Built in `/tmp` and copied in; `cmp` confirmed the store copy.

## Rule adopted

Every store file this worker writes from now on is built under `/tmp` and copied in (`cp`), never written whole in place: a whole-file write destroys the old content before the new one is durable.

## Second loss, 2026-09-16 ~12:20 UTC (same day)

- **12:10–12:17 UTC** — QA scenarios read `internal/parity/arbosdriver.py` successfully (the `xp-*` desktop probes ran from it).
- **12:23:40 UTC** — a scenario found `internal/parity/` gone; **12:24:52 UTC** — `ls` of the store root: `internal/ media/ notes.md` (no `docs/`); `internal/` down to 15 entries: `docs/`, `internal/mirror-docs.sh`, `internal/features-inbox/`, `internal/parity/`, `in-app-updates-and-signing.md`, `macos-worker-recipe.md`, `mobile-*.md`, `prime-intellect-harness-research.md`, `projects-post-gaps-2026-09-15.md` and others gone. `internal/qa/` intact (172 bug files, 344 rollouts), `notes.md` and `media/` present.
- The mirror branch `store-docs` had been pushed by the loop's 11:00 cycle (`bdb3578f`, 19 docs, 11:00:17 UTC): everything in `docs/` as of 11:00 was safe; nothing of QA's was written to `docs/` after 10:51.
- **12:26 UTC** — restored from the branch, via `/tmp`: all 19 `docs/*.md` (byte-identical to the branch) and `internal/mirror-docs.sh` (the alarm itself had been deleted). Then `mirror-docs.sh` pushed again so the branch carries the current `notes.md`.
- Not restorable from the mirror (out of its scope, other owners): `internal/parity/`, `internal/features-inbox/`, and the `internal/*.md` files above. The QA harness's desktop scenarios need `internal/parity/arbosdriver.py`; until it is back the loop uses the repo's copy at `desktop/driver/arbosdriver.py` (`ARBOS_QA_DRIVER_DIR`).
- Two losses in five hours, both hitting `docs/` and top-level `internal/` entries while `internal/qa/` (deep, busy) survived: the pattern points at the store service, not at any writer.
