---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Polish cycle 1 kernel asks — PR #185, branch `cursor/polish-kernel-asks-b027` (on `main`)

Four kernel changes from the layout worker's `internal/features-inbox/2026-09-14-polish-cycle-1-kernel-asks.md`.

1. **`plan add` de-dups.** A second `plan add` whose `[label](target)` names a target already on the list (paths compared without `./`, `.arbos/`, trailing `/`; URLs as written) rewrites that row in place and reopens it. No link on either side: same label (case-insensitive) counts. Tool result reads `Rewrote item n (it already named that target); nothing was added.` Also: `Added item n` now reports the item's place in the file, not the count (was wrong when the section was not last).
2. **Archive retires the page row.** When a worker is archived, every open row on `.arbos/notes.md` that targets `agents/<id>` (or a file under it) is rewritten: turn ended well → `- [x] [label](archive/agents/<id>) — worker finished: <last words>`; stopped/failed → stays `- [ ]` with `worker stopped: <why>`, link moved the same way. `kernel.log` line `page_rows_retired`. Needs the archive step (default on; `archive_children = false` skips it).
3. **Slug names read as words.** `spawn name:"math-docstrings"` → id `math-docstrings`, `agent.md` name `Math docstrings`. A name with spaces or capitals is kept as given.
4. **Spoken status.** A step whose whole text is one line `status: <words>` (any case, `**`/backticks stripped) sets the agent's status (source `agent`, a `status` frame, `status.toml`); the transcript keeps the line as the model wrote it. Two lines, or words before `status:`, are prose.
5. **Nudge `reason`.** `nudge` lines carry `reason` (`project page not updated`, `empty reply`, `tool call written as text`) beside `text`.

Scenarios:
- Coordinator adds `[Edge review](agents/math-edge-review) — worker running`, spawn fails, adds `[Edge review](agents/math-edge-review) — pending retry` → one row, second readout. `plan show` numbers still match `plan check n`.
- Worker with a row on the page finishes and is archived → row is `[x]`, links `archive/agents/<id>`, readout starts `worker finished:`; the panel's Project section no longer says "worker running". Root's next `plan check n readout target:<PR>` still rewrites it.
- Worker stopped with `say mode=stop` → row stays open, `worker stopped: …`.
- Replay reply `{"content":"status: Running sleep 45","calls":[bash]}` → `status` frame `step = "Running sleep 45"`, `source = "agent"`; transcript assistant line unchanged; idle clears it.
- `spawn name:"math-docstrings"` → desktop row and done line read "Math docstrings", not "Delegate 1" (with #183 or without).
- A transcript from before this PR (nudge lines without `reason`) still loads.

E2e: `crates/arbos-kernel/tests/archive_children_e2e.rs::archiving_a_worker_retires_its_row_on_the_project_page`, `status_e2e.rs::a_status_written_as_a_one_line_reply_sets_the_live_line`. Unit: `notes::tests`, `status::tests`, `hooks::spoken_name_tests`.
