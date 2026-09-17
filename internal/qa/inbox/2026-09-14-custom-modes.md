---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Custom modes (`/mode <skill>`) — PR #166, branch `cursor/custom-modes-b027` (on `main`)

T3-03. `/mode <skill>` typed at a chat pins the skill (agent.md `skill:`); the prompt carries "Mode: <name> …" plus the SKILL.md body every turn; `/mode off` clears; `/mode` reports/lists. A setting, not a prompt: notice on the transcript, no turn.

Scenarios:
- With a `haiku` skill: `/mode haiku`, then a prompt → the reply follows the skill (model run). `arbos-kernel prompt <place> --agent root --dump` shows the Mode block.
- `/mode nosuch` → notice with the skill list; nothing pinned.
- Delete the skill folder while pinned → the prompt says "pinned, but no skill of that name is here now".
- Fork a pinned chat → the fork's agent.md is fresh (no skill) — say if the mode should carry over.
- Desktop: no chip yet (composer follow-up); the notice is the only sign.

E2e: `crates/arbos-kernel/tests/custom_mode_e2e.rs`.
