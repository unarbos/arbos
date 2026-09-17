---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Stop words interrupt; queued follow-ups are not "Plan"

Branch `cursor/stop-word-queue-row-b027` (on integration `a8cabfc`). From Jacob's Mac session (`media/layout/jacob-stop-queued-as-plan.png`): "stop" typed during a turn was queued and the strip read "Plan · 1 message queued".

1. **Stop words.** A message that is only `stop`, `cancel`, `halt`, `wait`, or `pause` (case-insensitive, optional trailing `.`/`!`/`…`, surrounding spaces), sent while the chat is busy, interrupts exactly like the Stop button and posts `Stopped by you`. Never queued, never sent as a prompt. Two layers: the desktop (`submit` → `cancel` + notice) and the kernel (a `user` frame with a stop word for an agent that is running → `stop_work` + a `notice` line `Stopped by you`), so a desktop that misjudged "busy" (a running job, a lost socket) still gets the stop. While idle, the same words go to the model as a normal prompt.
2. **Queued follow-ups.** Inbox nodes (`inbox: true`, pending) leave the Plan strip. They get their own row set: `1 follow-up queued` / `N follow-ups queued`, one row per message with its text and three controls: **Send now** (cancels the node and steers the text into the running turn — Force), **Edit** (cancels the node, puts the text in the composer), **Remove** (cancels). "Plan" appears only when real nodes exist (steps, standing, questions, failed).

## Attack surface

- `Stop.`, ` STOP `, `stop!`, `stop…` → interrupt; `stop the build` → a prompt (queued while busy); `stop` while idle → a prompt.
- A stop word while a sub-agent runs but the parent is idle (the parent's `busy()` is false → the kernel path: `stop_work` stops the children too — is that what "stop" should mean? Today the Stop button does the same).
- A stop word while the desktop's socket is lost: the desktop cancels locally; the kernel never sees it; check the notice and the turn state on reconnect.
- Two follow-ups queued, Send now on the second (order), Edit on the first while a turn runs (composer gets the text; node cancelled), Remove while the node is being claimed by the kernel at that moment (race → it may fire anyway; the kernel's `plan_op cancel` answers with an error frame if the node is no longer pending — shown as a failed notice now).
- The local queue (prompts the desktop holds when the socket is down) and the kernel's queue can both be non-empty: two similar row sets; wording matches.
- The strip's `plan-head` id and the parity JSON: the "Plan" header is absent when only follow-ups exist — anything in QA scenarios counting on it?
