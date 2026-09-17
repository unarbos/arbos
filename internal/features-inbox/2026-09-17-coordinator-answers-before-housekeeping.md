---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Asked a question, the coordinator did its housekeeping and never answered

**From:** Jacob's desktop feedback `2026-09-17-9` — *The question was "waht is your git status" the agent response "Corrected the context" whould just give me the answer right?* Build 0.2.0 (1335), kernel `f9b6089c0b80`, `anthropic/claude-fable-5.1`, place Arena-GPS. Report: `media/desktop-feedback/2026-09-17-9/`.

## The turn (quoted from the trajectory)

```
user        what is the git status
tool bash   {"command": "git status", "description": "Show git status"}
thinking    The branch is clean and up to date with origin/main, with only two untracked
            .DS_Store files. I also see there's actually an origin remote configured, so
            I'll correct my earlier statement about that.
tool edit   {"path": ".arbos/docs/project-context.md",
             "old_string": "one commit, no remote configured;",
             "new_string": "one commit, tracks `origin/main`;"}
assistant   Corrected the context note to say the branch tracks `origin/main`.
turn_complete
```

The model had the answer in its thinking ("clean, up to date with origin/main, two untracked .DS_Store files") and said none of it. Its one line of prose was about the file it edited.

## Why the desktop cannot cover this

The Project chat hides the coordinator's tool calls, as Cursor's does — so the `git status` output was hidden work and the only visible thing was *Corrected the context note*. The desktop drew the turn correctly; there is nothing on the pane to fix. The question is answered only if the model says the answer.

## Ask

A line in the coordinator's prompt to the effect of: **answer the person's question in prose first; housekeeping (context notes, plan updates, the project page) comes after the answer, never instead of it.** The trajectory shows the model noticing its note was stale and treating the correction as the reply.
