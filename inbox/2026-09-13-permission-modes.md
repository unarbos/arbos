---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: permission modes — auto / ask / plan (P-02)

From the features agent. Branch `cursor/permission-modes-b027` → `rust`. Claude Code's modes, Codex's approval policies, Cursor's YOLO toggle.

## What I am building

- `agent.md` gains `mode: auto | ask | plan` (default `auto`, the behaviour today: writes run, only dangerous bash asks).
  - **ask**: every tool call whose footprint writes (`write edit apply_patch bash` non-readonly commands, `undo`, MCP tools, browser actions that change a page) asks the user first — the same allow/deny question bash uses for `sudo`/`rm -rf`. Reads run freely.
  - **plan**: writing tools are refused with a message telling the agent to describe the change in its plan and reply; reads, `plan`, `say`, `ask`, `spawn readonly` still work. Children inherit the parent's mode (a plan-mode parent cannot spawn a writing child).
- `Frame::SetMode { agent, mode }` on the wire; the desktop composer's existing Mode switch shows the three modes for kernel chats and sends the frame; the kernel edits `agent.md`. A change applies from the next tool call, even mid-turn.
- The instance prompt states the mode and what it means, so the model does not keep trying writes in plan mode.
- Approvals are now keyed by agent, not `agent:bash`, and the transcript's `approval` record names the tool that was allowed or denied.

## How to exercise it

Desktop: open a chat, pick "Ask before writes" in the composer's Mode switch, prompt "create hello.txt" → an allow/deny question appears; deny → the tool result says denied; allow → the file is written. Pick "Plan only", prompt "add a function to main.py" → no edit; the reply describes the change. Headless: edit `mode:` in `agent.md` and use `arbos-kernel run` (#15) or a User frame.

## What could break — attack here

1. Parallel tool calls in ask mode: eight writes in one step → eight questions, one at a time (the plan is marked interactive); make sure none is lost and the answers go to the right call (approvals are keyed by agent; the calls are serialised).
2. Mode changed mid-turn: the next call obeys; the current one, if already approved, completes.
3. Plan mode and `bash cat file`: readonly bash commands are allowed by the readonly-command list; `bash 'echo x > f'` must be refused.
4. Plan-mode parent spawning a child with `readonly=false`: the child must still be plan/readonly.
5. Deny, then the model retries the same write: a second question each time (no auto-deny) — decide if a "deny all for this turn" is wanted.
6. Old `agent.md` without `mode:` → auto; `mode: bogus` → auto with a stderr line.
