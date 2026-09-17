# K-14 transcript bloat: grep into `.arbos/`, uncapped tool bodies, tail re-reads — QA note (features agent, 2026-09-13)

Branch `cursor/transcript-bloat-b027`, base `rust`. Found while testing on the parity place: after ~50 agents, the kernel took >30 s to answer a connection and the desktop showed "Connection failed" / "Stopped."

## Root cause (measured)

`grep pattern="def " glob="**/*"` from a sub-agent matched inside **other agents' `transcript.jsonl`** files, which already held earlier grep results. The tool body was **146 MB**, stored whole as one transcript line (the transcript is the full-body store by design). Three agents did this: `.arbos/agents/` reached 872 MB. The kernel's tail loop re-reads every agent's whole transcript every 200 ms, so it never got to answering the socket.

## What the fix does

1. **grep/find skip `.arbos/`** (and `.git/`, `node_modules/` were already skipped by tgrep's gitignore handling; `.arbos` is now added) unless the search path itself points inside `.arbos/`. An agent that wants its own transcript says so (`path: ".arbos/agents/<id>"`), as the CONTRACT already suggests for `grep it`.
2. **Tool bodies are capped in the transcript at 1 MB.** Over that, the full body goes to `.arbos/agents/<id>/results/<call_id>.txt` and the transcript line keeps the first 64 KB plus `[… N MB more in <path>; read it in pieces]`. The model already saw an evicted view; the cite now points at the spill file.
3. **The tail loop tracks byte offsets** instead of re-parsing whole files: `load_transcript` is replaced there by reading from the last seen byte and parsing only new lines.

## Attack ideas

1. `grep path=".arbos/agents/root" pattern="…"`: still searches the transcript (explicit path inside `.arbos`).
2. `find name="*.jsonl"` at the place root: no `.arbos` hits.
3. A 1.5 MB `read` of a real file: spill file written, transcript line has the head + cite; `read` on the spill path works (it is under the agent dir, inside the place).
4. Two spills with the same call id (retry): file overwritten, not appended.
5. Tail offsets after a compaction rewrites the transcript (file shrinks): offset resets to 0 when the file is shorter than the offset; no duplicate broadcast? — check with `/compact`.
6. A half-written last line at the offset boundary: the reader keeps the partial and waits for `\n`.
7. Performance: 50 agents × 5 MB transcripts: kernel idle CPU should be near zero (was a full re-parse per tick).
8. The desktop's own `load_transcript` on open (kernel.rs replay) still reads whole files — fine once bodies are capped; note.
