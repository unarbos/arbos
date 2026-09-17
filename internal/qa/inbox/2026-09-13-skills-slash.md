# P-03 skills / slash commands — QA note (features agent, 2026-09-13)

Branch `cursor/skills-slash-b027`, base `rust`.

## What it does

- A **skill** is `<dir>/<name>/SKILL.md` or `<dir>/<name>.md` in `.arbos/skills`, `.agents/skills`, `.cursor/skills`, `skills`, `.arbos/prompts`, `~/.config/arbos/skills`, `~/.config/arbos/prompts` (first folder wins on a name clash). Front matter `name`/`description` optional.
- A user message (or a `say`/inbox text — anything that becomes a User event) that starts with `/name args` where `name` is a skill: the transcript keeps the text as typed; the model's copy gets `[skill name — path]` and the body under it. `$ARGUMENTS` in the body takes the words after the command; `$1`…`$9` take one each; a body with neither gets `Arguments: …` appended.
- The prompt's `Skills:` block lists `name — description` lines and says how to invoke one.
- The desktop already lists skills in the composer's `/` menu (`kernel::list_commands`); nothing changed there.

## Attack ideas

1. `/review` with no skill of that name: text passes through unchanged; the model should not invent a skill.
2. `/Review` (case): matched case-insensitively — check the roster still shows the file's casing.
3. Skill body of 300 KB: it is appended to the user message with no eviction — the model gets it whole. Probably fine once; check compaction folds it.
4. `$ARGUMENTS` with a newline in the args (multi-line prompt): only the first line? `split_slash` takes everything after the name, trimmed, newlines kept.
5. Two skills `a/SKILL.md` and `a.md` in the same dir: the dir wins (sorted paths: `a` < `a.md`).
6. Skill in `~/.config/arbos/skills` vs place skill of the same name: place wins.
7. A skill whose body itself starts with `/other`: no recursion (expansion happens once, on the user text only).
8. `/name` from a child agent's `say` (an inbox message that becomes a User event with `[from]` prefix): does the prefix defeat `starts_with('/')`? Expected: yes, so peers cannot trigger skills — confirm and decide.
9. A skill named `compact` or `undo` (desktop builtins): the desktop answers those itself; the kernel would also expand. Note the shadowing.
10. `.cursor/skills/<name>/SKILL.md` from a real Cursor repo: parses; description shows.

## How to run

```
mkdir -p .arbos/skills/review && printf -- '---\ndescription: Review a file for bugs.\n---\nReview $ARGUMENTS. Read it, list bugs as path:line bullets, do not edit.\n' > .arbos/skills/review/SKILL.md
arbos-kernel run . '/review math.py'
```

Expect the agent to read `math.py` and list bugs without editing; the transcript's user line is `/review math.py`.
