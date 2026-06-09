# ADR-0031 — Interactive TUI and edit display-diff

- Status: Accepted
- Date: 2026-06-08

## Context

pi's coding agent is primarily an interactive terminal app with differential
rendering (pi-tui), and its edit tool shows a line-numbered display diff. arbos
had only headless surfaces (one-shot, control seam). The TUI is the last
first-class surface, and it needs the edit diff to render.

## Decision

- Edit display-diff: `codingspec.generateDiffString` computes a line-level LCS
  diff and renders pi's view — changed lines marked `+`/`-` with new/old line
  numbers, surrounded by context, long unchanged runs collapsed to `...`. The
  edit tool now carries it in its (model-invisible) `Details.diff`; the model
  still sees only the plain success message.
- TUI: `cmd/arbos-tui`, a Bubble Tea program. It is just another consumer of the
  `Conversation` event stream — prompt line plus a streaming transcript that
  renders assistant text, tool calls, and the edit diff (colored from
  `Details.diff`). It drives the same `pi.NewEngine` as the headless surfaces, so
  behavior is identical across modes. With no endpoint it uses the fake provider.

## Consequences

- arbos ships pi as a first-class coding agent across all three surfaces:
  interactive TUI, headless one-shot, and control-seam RPC.
- No kernel change: the TUI reads existing events; the diff rides the existing
  `ToolResult.Details` (ADR-0021). Bubble Tea / bubbles / lipgloss are added as
  dependencies (TUI-only).
- generateDiffString is the one place this phase uses the granted unit-test
  exception (diff edge cases are impractical to assert purely at runtime). The
  TUI diff render was proven end-to-end: the real binary, driven under tmux
  against a scripted OpenAI endpoint, ran the real edit tool and rendered the
  line-numbered diff while the file changed on disk.
- pi-tui's differential renderer is not ported byte-for-byte; Bubble Tea provides
  equivalent diff-based terminal rendering, so reproducing pi's custom diff engine
  would be redundant (subtract-before-you-add).
