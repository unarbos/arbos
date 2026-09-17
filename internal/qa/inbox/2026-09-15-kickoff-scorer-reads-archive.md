---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: the kickoff scorer misses archived workers (items 3, 4, 7), and item 3's fix — PR #212

Two kickoff runs on `main` today (`/tmp/arbos-qa-rollouts/20260915T014154Z-…`, `…T020342Z-…`, kernel from `main` and from `cursor/show-output-image-b027`) both score item 3 `ran toy-repo: False` although the toy-repo worker ran `python3 hello.py` twice, and item 4 `0 fetch/search calls` although the research worker fetched. Cause: `all_agent_tools` in `run.py` globs `.arbos/agents/*/transcript.jsonl`; since #144 finished workers move to `.arbos/archive/agents/<id>/` at the end of their turn, so by the time the scorer runs every worker's transcript is there and `every` falls back to root's tools only (the evidence says `agents now ['root']`). Item 7's `say` count is affected the same way when the steer's reply lives in the worker.

Ask: read both `agents/*/transcript.jsonl` and `archive/agents/*/transcript.jsonl` in `all_agent_tools` (and wherever else worker transcripts are read for evidence). The `shots` count already walks all of `.arbos/` and found the image.

## Item 3 itself (PR #212)

With the scorer fixed, item 3 passes on `#212`'s kernel. What was wrong, from the worker transcripts:

1. The Show line (#141) was left out of the toy-repo brief because the coordinator's `do` step said "Capture output." and the kernel read "capture" as "the brief already asks for an image". Fixed: only picture words count.
2. With the Show line, the worker had no tool for "terminal output as an image" on a headless machine and tried `script` + ImageMagick `convert label:@file`, refused by ImageMagick's policy. Fixed: `screenshot target:text title:"…" text:"…"` renders the words with headless Chrome into `images/`; CONTRACT/PROTOCOL/the Show line name it.

In the last run the worker called it on its first try and root's reply to the user names the path; still: `media/features/show-output-image-kickoff-item-3.png`.

Scenario for the suite: a coordinator place, no `DISPLAY`, user says "run X and show me the output" → the worker's brief has `Show:`; the worker's transcript has a `screenshot` call with `target: text` and no error; a PNG exists under `.arbos/(archive/)agents/<id>/images/`; root's reply contains that path. Without Chrome the tool errors with "needs chromium or google-chrome"; the scenario should skip there like `browser_e2e`.
