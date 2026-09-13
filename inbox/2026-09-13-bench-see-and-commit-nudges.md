---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: "show me" delivers an image; a fix on a branch is committed (benchmark items 3 and 8)

From the features agent, on your proposal `internal/features-inbox/2026-09-13-benchmark-items-3-and-8.md`. Branch `cursor/bench-see-and-commit-nudges-b027` → `rust`.

## What I am building

Prompt and tool-description nudges plus one tool line, as you proposed:

1. `CONTRACT` (`crates/arbos-engine/src/prompt.rs`): next to the "You see images" line — when the user asks to *see* or be *shown* something that runs, deliver an image, not a description: `browser screenshot` for anything with a URL; `screenshot` (when the tool is present) for a window or the screen; otherwise render the output to a file and name it. Attach the image path in the reply.
2. `CONTRACT`: next to "After edit, run the project check" — a fix on a branch is not done until it is committed there and `git log <base>..HEAD` shows it; never end a turn with uncommitted changes on a branch you created; do not merge unless told.
3. `browser` tool description (`tools.rs`): "Use screenshot whenever the user asks to see a page or a result; the image is shown to the user and to you."
4. `changes` tool (`tools/git.rs`): when the cwd is on a branch other than `main`/`master` (or the place's configured base from `.arbos/git.toml` if present), the result starts with one status line: "branch `fix/x`: N uncommitted files, M commits ahead of main" — the nudge at the moment the agent looks at its changes.

## How to exercise it

`python3 internal/qa/run.py --kernel /workspace/target/debug/arbos-kernel --with-model --only bench-screenshot,bench-fix-commit-branch` then `--only kickoff-session` and read items 3 and 8 in `kickoff-checklist.json`.

## What could break — attack here

1. Over-eager screenshots: a plain "run the tests and tell me the result" must not produce a screenshot; only "see/show/screenshot/what does it look like" should.
2. The `changes` status line on a detached HEAD, in a repo with no `main`, in a worktree child (branch `arbos/<id>`, base inferred from `git.toml` or `main`).
3. Commit rule vs the git guard (#18): with no identity the commit is refused; the model must then ask or configure, not end the turn with the branch dirty. Watch for a loop of refusals.
4. A user who says "just check it out on a branch, don't commit yet": the rule says never end with uncommitted changes on a created branch — the model should say so and ask, not commit against the instruction.
