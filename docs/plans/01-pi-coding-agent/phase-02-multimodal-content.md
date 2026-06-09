# Phase 2 — Multimodal message content and vision

Back to [overview](./overview.md). Foundational (scaffold). Decisions D3 and D8.
Advances ADR-0012. Reuses `ContentBlock` from Phase 1.

## Goal

Let a message carry image content so read can return images and a vision-capable
model can see them, additively.

## Current shape and the trace

`core.Message` is `{ Role; Content string; Reasoning string; ToolCalls; ToolCallID; ProviderMeta }`
(`internal/core/message.go`). Readers of `Content`:

- `Project` builds system and fenced-context messages from strings
  (`core/project.go`).
- `ProjectEvent` maps message and tool payloads to `Content`.
- `openai.toWireMessage` sets wire `content` from the string.
- `engine` builds user and assistant messages with string `Content`; the token
  estimator (`engine.estimateTokens`, `compaction`) sums `len(Content)`.

Pi's own message content is `string | (TextContent | ImageContent)[]` (pi-ai
`UserMessage`/`ToolResultMessage`). The additive Go shape matches that union: a
string when simple, blocks when multimodal.

## Target type design (sketch)

```go
type Message struct {
    Role         Role
    Content      string         // unchanged: canonical text
    Parts        []ContentBlock // NEW: non-text parts (images), reuses Phase 1 type
    Reasoning    string
    ToolCalls    []ToolCall
    ToolCallID   string
    ProviderMeta map[string]json.RawMessage
}
```

Invariant. `Content` holds all text; `Parts` holds non-text blocks in order. A
consumer wanting the full multimodal view interleaves `Content` as a leading text
block with `Parts`. The OpenAI adapter sends a plain string `content` when
`Parts` is empty (today's path), or a content array `[{text: Content}, …images]`
when `Parts` is set.

## Changes

- Add `Message.Parts`. `ProjectEvent` copies `ToolResult.Blocks` (Phase 1) into
  the tool message's `Parts`, and a user message with images carries them in
  `Parts`.
- Teach `openai.toWireMessage`/`buildRequest` to emit a content array when
  `Parts` is set, and advertise `Capabilities.Vision` for vision models.
- Token estimator adds a per-image constant (pi uses ~4800 chars per image).
- Amend ADR-0012 to record multimodal content is now in.

## Why additive

`Parts` is a new `omitempty` field; old `MessagePayload` rows decode with `Parts`
nil. No version bump. The wire change is localized to the adapter.

## Open edge

Some endpoints reject image content in a tool-role message. The adapter falls
back to a text note for tool-role images on non-supporting providers. Flag during
implementation.

## Dependencies

Phase 1 (the `ContentBlock` type).

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Control-cli runs a turn carrying an image part and asserts the adapter
serializes a content array; a text-only turn serializes a string `content`
exactly as today.
