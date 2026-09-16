---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Prompt-size pass — PR #119, branch `cursor/prompt-size-b027` (base: integration head)

The system prompt every model call carried was ~7k tokens for a fresh coordinator and ~10k for a worker with a little history. This PR makes it smaller without dropping a rule: the contract is compressed, the long form lives in `.arbos/PROTOCOL.md` (the kernel writes it), rosters list names only, tool schemas are terse, and the kernel logs one `prompt_size` line per turn.

## Numbers (real o200k tokens of the traced request body, same place and message)

| agent | before | after |
|---|---|---|
| coordinator root, fresh place | system 3217 + tools 3736 = 6997 | system 1878 + tools 2312 = **4247** |
| plain worker (25 tools), system+tools only | 2275 + 4398 = 6673 | 1408 + 2732 = **4140** |

Coordinator: under the 5k target. Plain worker: 4.1k, not 3k — 25 tool schemas cost ~2.7k even terse (about 35 tokens of JSON structure per tool before any words). The rest of the way needs a decision on the worker's default tool roster (see the PR body); not taken here.

## What to test

- `arbos-kernel prompt <place>` and `--agent <id>`: sections and per-tool token estimates; `--json` for a machine-readable form; `--dump` prints each section's text as JSON lines (for a real tokenizer). Check the total matches the `prompt_size` line the kernel logs for that agent's next turn (`system=` should equal contract + project-context + instance + plan).
- `prompt_size` in `.arbos/runtime/kernel.log`: one line per turn, `system= tools= conversation= total=`. After the provider reports usage the numbers are calibrated (they may move a little between turns).
- `.arbos/PROTOCOL.md` appears at kernel start in every place; `arbos-kernel check` warns when it is missing or differs from the build's text (an older kernel wrote it). Edit it by hand, restart the kernel: it is rewritten.
- Rules kept: ask a model for the things the old long contract said and see that the behaviour holds — a timer request becomes a subscription (no bash sleep), `say` for another agent's message, secrets never printed, tests never weakened, commit before the turn ends. When something is unclear the model should `read .arbos/PROTOCOL.md` — a rollout that shows that read is a good sign, not a bad one.
- Tool schemas: rare fields still work though the schema does not list them — `spawn model:… readonly:true cwd:… wait_secs:…`, `subscribe at:":15"`, `subscribe expires:…`. `plan set items:[…]` still takes strings, `{section, text}`, nested `{goal, children}`, or a markdown checklist string.
- Rosters: with kinds, machines, skills configured, the prompt shows names only plus the folder to read. `arbos-kernel prompt` shows the instance block; the `Kinds`/`Machines`/`Skills` lines should be one line each.
- A worker with a long history: `prompt_size` shows `conversation` growing and `system`/`tools` flat.

## Kickoff benchmark

`run.py --only kickoff-session --with-model` (gpt-4.1-mini), same day, alternating binaries: integration head `c9f8c60` → 6, 7 of 12; this branch `e6733fb` → 5, 7, 7. Same range. History lines to read with care: three under `cursor/prompt-size-b027` at 19:49–20:08 (4, 5, 6) were before the two spawn fixes in the PR (the model wrote `host="local"` and `isolate=worktree` in a non-repo place; both spawns failed), and the one at 20:26 ran a different binary by mistake — treat it as a baseline number.

Scorer note: items 2 and 11 look for markdown outside `.arbos/` (`".arbos" not in p.parts`). Since #98/#107 the coordinator writes `.arbos/docs/project-context.md` and `.arbos/notes.md`, so a correct run fails both on either kernel (see the seam note `internal/features-inbox/2026-09-13-prompt-size-seams.md`).

## Known

- The `prompt_size` line uses the engine's chars/4 estimate calibrated by the provider's last report; `arbos-kernel prompt` uses raw chars/4. Real tokenizers count dense JSON higher and prose lower; the traced request body (`trace = true` in config) is the exact thing.
- One test changed: `hub::tests::roster_round_trips_through_the_folder` pinned the old machine line; it now checks the name and the pointer (the task asked for names).
