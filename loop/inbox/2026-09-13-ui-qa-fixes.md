---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# UI QA pass fixes (ui-001…005, 006, 007, 010, 011) — branch `cursor/ui-qa-fixes-b027`

Base: integration head (`99c0174`). One PR.

## What changed

- **ui-001 Stop shows "no reply from the kernel"** — already fixed on the integration head (the `interrupt_label` "stop…" clause and `stop_requested`). Verified three ways on this branch: Stop during a tool call, Stop mid-stream, Stop before the first token (all give one `Stopped by you`, no failed notice). `ui_pass.py --phases T`: `stop-notice`, `stop-word-notice` pass.
- **ui-002 Rewind here does not return the prompt** — already fixed on the head; verified: composer text is the prompt, notice says `project back to <sha>` after an edit turn. `rewind-turn` passes.
- **ui-003 Skip sends an empty answer** — kernel: the `ask` tool turns an empty answer into "The user skipped this question without answering. Choose a sensible default yourself, say which you chose, and continue." Verified: model replies "I'll use alpha."
- **ui-004 Skip sent ask id "ask"** — desktop: the transcript-tail `ask` line now carries the kernel's `call_id` (or the agent id, blind); the placeholder `"ask"` is gone. A repeat of a question answered in the last 10 s (the tail replaying the live frame) is ignored, and the memory resets on every new prompt. Kernel: an id it never issued is taken when exactly one question is pending (logged `answer_id_unknown`) instead of refusing and leaving the turn hung.
- **ui-005 Plan strip Stop leaves "1 standing"** — a stopped standing node is `blocked`; the header now says `1 paused`, the Stop control goes until something is armed again, the row keeps Run/Cancel. Screenshot `media/features/ui-qa-fixes/plan-strip-paused-after-stop.png`.
- **ui-006 no Edit for queued follow-ups** — `edit-queue-N` puts the text back into the composer and drops it from the queue (`Interrupt now · Edit · ×`).
- **ui-007 Mac wording on Linux** — mic tooltip "Dictation (macOS only for now)", error "voice dictation is macOS-only for now", opener row "This machine".
- **ui-010 Settings › Model disagrees with the kernel** — the API key row adds a line with the kernel's own `provider` frame: "The kernel of the open chat answers with openrouter (model); key from env:OPENROUTER_API_KEY." Could not reproduce the Custom-endpoint reading with the pass's config (`api_base` + `api_key_env`, no `provider`): it infers OpenRouter. If you see it again, attach the `config.toml` (redacted).
- **ui-011 dead Local row** — `composer-machine` opens the folder picker (OpenProject) at its machine step.

## How to check

`internal/parity/ui_pass.py --phases LQPSAT` on this branch: T all pass (`stop-notice`, `stop-word-notice`, `rewind-turn`, `force-queue`); Q's Skip path needs the model to ask twice — gpt-5.4-mini asked once in my run. P: `plan-head` unreachable because gpt-5.4-mini could not format `when` for the plan tool (4 rejected calls, "choose one of after, every") — a prompt/tool-schema gap the plan.md rewrite retires; use `Call the plan tool exactly once with {"op":"add","parent":0,"nodes":[{"goal":"…","when":{"every":"1h"},"do":{"shell":"python3 main.py"}}]}` to get a standing node deterministically.

Known not fixed here: `followup-*` element ids (the pass looks for them; this desktop's rows are `force-queue`/`edit-queue-N`/`unqueue-N`).
