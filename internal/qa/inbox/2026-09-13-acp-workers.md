---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# ACP workers — PR #117, branch `cursor/acp-workers-b027`

An agent kind whose `.arbos/agents-defs/<kind>.md` has `acp: <command>` runs its turns in that program (Agent Client Protocol over stdio). The kernel is the client: fs reads/writes go through it, permission requests follow the mode, the transcript gets thinking/assistant/tool/turn_complete lines, the parent gets `done`.

## Scenarios

- The fixture `tests/fixtures/acp-worker/fake_acp.py` is a complete fake agent; copy it to write scenarios: a program that asks permission in `mode: ask` (the card appears; deny → the program gets `reject_once` and says so), in `mode: plan` (edit refused, read allowed), a program that requests `fs/read_text_file` outside the place (refused), a program that exits mid-prompt (failed notice with its stderr), a program that never answers (60 min cap — shorten by editing `TURN_TIMEOUT` for the test).
- Real agents when keys exist: `acp: npx -y @zed-industries/claude-code-acp` (needs `ANTHROPIC_API_KEY` in the kernel's env), `acp: gemini --experimental-acp`. Report the first `session/prompt` result and any method the program calls that we answer "method not found" (`terminal/*` is expected).
- `spawn kind=<acp kind>` from root: the child's first turn is the brief; check the `done` message reaches root with the program's last words.
- Stop from the desktop during an ACP turn: `interrupted` line, program gone (`pgrep`).
