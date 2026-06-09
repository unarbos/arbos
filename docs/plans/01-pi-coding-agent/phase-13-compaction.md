# Phase 13 — Compaction intelligence

Back to [overview](./overview.md). Decision D5 (core, not optional). Mirror
`coding-agent/src/core/compaction/compaction.ts`.

## Goal

Bring across pi's context-management intelligence, not arbos's marker
compression. Real coding intelligence for long sessions.

## pi behavioral spec

Settings (`DEFAULT_COMPACTION_SETTINGS`): `enabled: true`, `reserveTokens: 16384`,
`keepRecentTokens: 20000`.

Trigger (`shouldCompact`): compact when `contextTokens > contextWindow -
reserveTokens`, where `contextWindow` comes from model metadata (Phase 3) and
`contextTokens` is the last assistant usage plus an estimate of trailing
messages.

Cut point (`findCutPoint`). Valid cut points are user, assistant, custom,
bashExecution, branch-summary, and compaction-summary entries, NEVER a tool
result (a tool result must stay with its assistant tool_call). Walk newest to
oldest accumulating estimated tokens until `keepRecentTokens` is reached, then
cut at the nearest valid point at or after that entry. If the cut lands
mid-turn (not at a user message), record the turn-start so the turn prefix can be
summarized separately.

Summary (`generateSummary`). Serialize the to-be-dropped messages to text, wrap
in `<conversation>…</conversation>`, and ask the model for a structured
checkpoint with this EXACT section layout: `## Goal`, `## Constraints &
Preferences`, `## Progress` (with `### Done`, `### In Progress`, `### Blocked`),
`## Key Decisions`, `## Next Steps`, `## Critical Context`. Closing instruction:
keep sections concise, preserve exact file paths, function names, and error
messages. Uses a dedicated `SUMMARIZATION_SYSTEM_PROMPT`.

Iterative update. When a previous summary exists, use the UPDATE variant in
`<previous-summary>` tags: preserve all prior info, add new progress and
decisions, move In Progress to Done as completed, update Next Steps, preserve
exact paths/names/errors. Same section layout.

Split turn. When cutting mid-turn, also summarize the turn prefix with the
turn-prefix prompt (`## Original Request`, `## Early Progress`, `## Context for
Suffix`) and merge as `…\n\n---\n\n**Turn Context (split turn):**\n\n…`.

File tracking. Extract read and modified files from tool calls across the
summarized span (and prior compaction details), then append the read/modified
file lists to the summary (`formatFileOperations`).

## Integration with arbos

- Implement a `ContextPolicy` whose `ShouldCompress`/`CompressibleRange` apply the
  trigger and the tool-pair-safe cut point, driven by `ModelInfo.ContextWindow`.
- Implement a `Summarizer` that emits the structured checkpoint and the UPDATE
  variant, with file tracking. arbos folds the span in place (ADR-0014); the
  summary renders at the span start via the existing projection.

## Data structures

`CompactionSettings{ Enabled bool; ReserveTokens, KeepRecentTokens int }`, reusing
`internal/compaction` and the `ports.ContextPolicy`/`ports.Summarizer` seams.

## Dependencies

Phase 3 (context-window metadata). Wired in Phase 15.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Harness runs a session past the budget against a scripted provider and
asserts: a compression event lands, the cut point did not split a tool_call from
its result, the summary follows the exact section layout, file lists are
appended, and a second compaction uses the UPDATE-merge form.
