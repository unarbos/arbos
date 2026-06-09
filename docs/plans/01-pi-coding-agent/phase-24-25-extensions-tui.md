# Phases 24–25 — Extension seam, TUI, edit display-diff

Back to [overview](./overview.md). Status: done. ADR-0030, ADR-0031.

## Phase 24 — Go extension seam + event bus

- `internal/extension`: build-time `Extension func(API) error`; `API` offers
  `RegisterTool`, `RegisterCommand`, `On(event, handler)`. The `Bus` is a
  `ports.Observer` mapping each KernelEvent to pi's event names (`agent_end`,
  `turn_end`, `message_update`, `tool_execution_start/end`, `tool_call`,
  `tool_result`, `error`). No kernel change.
- pi's `todo`/`subagent` are example extensions (not built-ins) and arbos's
  first-class `delegate`/`start_coding_session` already cover delegation, so
  nothing built-in-via-extension needed porting. A runtime TS plugin loader is
  intentionally not reproduced (incompatible with a static Go binary).
- Proof: an extension registered a tool (dispatched through the engine), a
  `tool_result` handler (fired), and a slash command (expanded).

## Phase 25 — TUI + edit display-diff

- `codingspec.generateDiffString`: line-numbered display diff (LCS, +/- with
  line numbers, context, `...` collapse), carried in edit's `Details.diff`.
- `cmd/arbos-tui`: Bubble Tea front-end consuming the Conversation event stream;
  renders streamed text, tool calls, and the colored edit diff. Same
  `pi.NewEngine` as headless surfaces.
- Proof: unit test for `generateDiffString`; the real TUI driven under tmux
  against a scripted OpenAI endpoint ran the real edit tool and rendered the diff
  (`-1 hello world` / `+1 hello arbos` + context) while the file changed on disk.
