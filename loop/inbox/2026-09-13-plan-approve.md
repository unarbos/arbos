---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Plan mode with approval — PR #112, branch `cursor/plan-approve-b027` (on #107)

- Chat in `plan` mode (`/mode plan` or the composer chip) + an idle turn + open checklist items → the plan strip header shows **Approve and run** (`plan-approve-<chat id>`). Click → mode `auto`, prompt "Plan approved. Execute your checklist now…". Nothing else changes: the strip's Run/✕ on checklist rows now check/remove items.

## Scenario ideas

- Plan mode, model writes the list, user edits an item (✕ one, type "add X too") → Approve → only the remaining items are worked.
- Approve while the turn is still running → the button is not shown (idle only); check it never sends during a turn.
- Approve with zero open items → no button.
- A child agent in plan mode: the button shows in its chat too; approving switches only that agent's mode (children inherit the stricter of parent and own — check the parent's `auto` does not get overwritten).
- `plan` mode + the model refuses to call `plan set` (gpt-5.4-mini did once) → nothing to approve; the directive text is the protocol worker's to firm up.
