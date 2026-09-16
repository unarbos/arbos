---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# "Show me" travels with the brief — PR #141, branch `cursor/show-me-brief-b027` (on `main`)

Kickoff item 3: root's kickoff said "run … and report" and the worker made no image. Now, when the user message that opened the spawning turn asks to see the result (`asks_to_see`: "show me", "let me see", "screenshot", "I want to see", "what it looks like", "see it running", …) and the brief itself does not mention a screenshot/image/capture, the kickoff gets a `Show` line between Output and Report:

> Show: The user asked to see this. An image of the result is owed: `browser screenshot` for a page, `screenshot` for a window, or the terminal output saved as an image under .arbos/media/<topic>/. Name its path in your report; words alone do not close the task.

A raw `brief` gets the same line appended. Judged only on the turn's own user text (the `user` line(s) after the last `wake`): a done- or timer-opened turn adds nothing, so an old "show me" does not haunt later spawns.

- E2e: `crates/arbos-kernel/tests/show_me_e2e.rs` — "…and show me the output" → line present, in place; "…fix what breaks" → absent. Unit tests for the phrase list.
- To re-run: `kickoff-session` item 3 with a model. Expect the worker's brief (its first `wake` text in `.arbos/agents/<w>/transcript.jsonl`) to carry `Show:` and the worker to save an image under `.arbos/media/`. Scorer side still needs to look in children's transcripts and `.arbos/media/` (scorer-gaps note).
- Phrase misses are the likely bug class: send me the user wording that should have counted.
