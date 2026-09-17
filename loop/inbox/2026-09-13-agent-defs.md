# K-10 custom agent definitions — QA note (features agent, 2026-09-13)

Branch `cursor/agent-defs-b027`, base `rust`.

## What it does

A **definition** is a markdown file that describes a kind of agent: which model, which tools, whether it may write, and its standing instructions. A parent spawns one with `spawn kind=<name>`.

Files: `.arbos/agents-defs/<name>.md` (Arbos) and `.cursor/agents/<name>.md` (read for Cursor compatibility; `.arbos` wins on a name clash).

Format: front matter between `---` lines, then the body.

```
---
name: reviewer
description: Reviews a diff for bugs and reports findings; never edits.
model: inherit
allowlist: ls, read, grep, find, bash, say, plan
readonly: true
---
You review code. Read the diff with `git diff`, ...
```

Keys: `name` (defaults to the file name), `description` (shown to parents in the prompt), `model`, `allowlist` (alias `tools`), `readonly`, `cwd`. Body = the child's standing instructions, copied to `.arbos/agents/<child>/instructions.md` at spawn and shown in its instance prompt.

Rules:
- `spawn kind=x` with an unknown `x` fails with the list of known kinds.
- The def cannot grant more than the parent has (`restrict_allowlist` still runs). `readonly: true` in the def wins over `readonly: false` from the spawn call.
- An explicit `model=` on the spawn call beats the def's `model`.
- `agent.md` gets a `kind:` line; the roster and tree show it.
- The instance prompt lists the kinds available (`Kinds: reviewer — Reviews a diff…`).

## Attack ideas

1. Def with `allowlist: bash, write` spawned from a readonly parent: child must stay readonly and lose `write`.
2. Def name with `/` or `..` in the file name or `name:` key: must be rejected or ignored, never used as a path.
3. Def body of 200 KB: instructions should be evicted/clipped like AGENTS.md, not blow the prompt.
4. Two defs with the same `name:` in different files: first by path order wins; note which.
5. Edit the def after spawn: the running child keeps its copied `instructions.md` (by design); check the prompt says so.
6. Def with `model: gpt-nonexistent`: child turn fails cleanly with the provider error, parent gets a `say` back or a Notice.
7. `.cursor/agents/foo.md` with Cursor-only keys (`is_background: true`): ignored without a warning storm.
8. Malformed front matter (no closing `---`): the whole file is treated as body, name from the file name.
9. `spawn kind=reviewer readonly=false`: still readonly (def wins).
10. Child of a `kind` child spawning with the same kind: works at depth 2, hits the depth cap at 3 with the usual message.

## How to run

`cargo build -p arbos-kernel`; create `.arbos/agents-defs/reviewer.md` as above in a test place; prompt the root with "spawn a reviewer for the last commit and wait for its report". Check `.arbos/agents/<child>/agent.md` has `kind: reviewer`, `instructions.md` exists, and the child's tool calls stay inside its allowlist.
