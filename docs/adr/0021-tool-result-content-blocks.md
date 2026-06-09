# ADR-0021 — Tool results carry multimodal content blocks and structured details

- Status: Accepted
- Date: 2026-06-08

## Context

`core.ToolResult` was string-only (`{ CallID, Content string, IsError }`). The
in-house coding agent port (docs/plans/01-pi-coding-agent) requires two things a
single string cannot hold. A `read` tool returns images. An `edit` tool returns
a display diff and a unified patch, and a `bash` tool returns a truncation record
and a spill-file path, all of which a renderer and the compaction summarizer need
but the model must not see.

The access-pattern trace found the only consumer of `ToolResult.Content` as a
string is `ProjectEvent` (`core/project.go`); the engine treats the result
opaquely and the store serializes the payload with `encoding/json`. That makes a
purely additive extension possible.

## Decision

Introduce a shared content-block shape and extend `ToolResult` additively,
keeping `Content` as the canonical text channel.

- New `core.ContentBlock` (`Type` text|image, with `Text` or `*ImageData`),
  `core.ContentType`, and `core.ImageData`. This is the single shape multimodal
  message content will reuse (the multimodal-message follow-up to ADR-0012).
- `core.ToolResult` gains `Blocks []ContentBlock` (non-text output) and
  `Details json.RawMessage` (structured, model-invisible), both `omitempty`.
  `Content` is unchanged and remains what the model sees.
- `tool.NewRichSpec` is added beside `tool.NewSpec`. String-returning handlers
  keep using `NewSpec` unchanged; handlers that return blocks or details use
  `NewRichSpec`. Both share one decode path so the arg type, decoder, and schema
  cannot drift.

The change is purely additive. `CurrentEventVersion` stays 1 and no upcast rule
is added: an old persisted `ToolResultPayload` with only `Content` decodes into
the new struct with `Blocks` and `Details` empty, and a result that sets neither
serializes byte-identically to before.

## Consequences

- `read` can return images and `edit`/`bash` can attach diffs and records once
  their phases land.
- Projecting `Blocks` into a tool message requires multimodal message content;
  until that phase, `ProjectEvent` continues to project `Content` only and the
  blocks ride in the event log. The OpenAI adapter gains block serialization in
  that same later phase.
- `Content` plus `Blocks` is a dual representation. The invariant is that
  `Content` holds all text and `Blocks` holds non-text parts in order; a consumer
  wanting the full multimodal view treats `Content` as a leading text block. This
  mirrors the `string | (text | image)[]` content union real providers expose, so
  the additive shape is also the faithful one.
- No store migration, no event-version bump, no change to existing tool handlers.
