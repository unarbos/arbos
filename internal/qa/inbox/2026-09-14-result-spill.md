---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Result spill — PR #169, branch `cursor/result-spill-b027` (on `main`)

T3-11. Any tool body past 24 KB or 200 lines is written whole to `.arbos/agents/<id>/results/<call id>.txt`; the model's evicted view cites that path (`read path offset` continues). `bash` keeps citing the job's `out.log`. Transcript lines stay whole up to 1 MB as before.

Scenarios:
- `fetch` a long page → `results/<call>.txt` exists; the next model step can `read` it at an offset (see the e2e's replay).
- `read` of a 5000-line file → the view is the head with the cite of the file itself (unchanged) — no spill needed since the source is a file; the results file is written anyway (harmless duplicate; say if you want `read` exempt).
- `grep` with thousands of hits → spill + cite.
- Archive a worker → its `results/` goes with the folder; `grep scope=history` still finds text in them? (It greps transcripts only — results/ files are not indexed; say if they should be.)
- Disk: a long session with many big fetches grows `results/`; the gc chore does not prune it. Candidate for a `check` note past N MB.

E2e: `crates/arbos-kernel/tests/result_spill_e2e.rs`; unit `project::spill_cite_tests`.
