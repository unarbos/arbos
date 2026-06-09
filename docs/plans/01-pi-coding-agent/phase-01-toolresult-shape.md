# Phase 1 — ToolResult content blocks and details

Back to [overview](./overview.md). Foundational (scaffold). Decision D3. Births
the shared `ContentBlock` type that Phase 2 reuses.

## Goal

Let a tool result carry ordered multimodal content (text and image) and a
structured, model-invisible `details` payload, so faithful read (images) and
edit/bash (diffs, patches, truncation records) have somewhere to live. Do it
additively so no persisted row changes shape.

## Current shape and the trace

`core.ToolResult` today is `{ CallID string; Content string; IsError bool }`
(`internal/core/tool.go`). Every read/write site:

- Written by `tool.Registry.Dispatch` (`internal/tool/registry.go`), which wraps
  a handler's `(string, error)` into `ToolResult{Content: out}`. Also `fake.Tools`.
- Persisted by `ToolResultPayload` via `json.Marshal` (`core/codec.go`).
- Projected by `ProjectEvent` (`core/project.go`):
  `Message{Role: RoleTool, ToolCallID: …, Content: p.Result.Content}`.
- Serialized to the wire by `openai.toWireMessage` (`provider/openai/openai.go`)
  as a string `content`.

The only consumer of `Content` as a string is `ProjectEvent`. That is the choke
point, which makes the additive design safe.

## Target type design (sketch)

```go
// ContentBlock is one piece of multimodal content. Tagged for clean JSON and
// wire round-trip. Mirrors pi's TextContent | ImageContent union.
type ContentType string
const ( BlockText ContentType = "text"; BlockImage ContentType = "image" )

type ContentBlock struct {
    Type  ContentType `json:"type"`
    Text  string      `json:"text,omitempty"`
    Image *ImageData  `json:"image,omitempty"`
}
type ImageData struct {
    Data     string `json:"data"`      // base64
    MimeType string `json:"mimeType"`
}

type ToolResult struct {
    CallID  string
    Content string           // unchanged: canonical text the model sees
    Blocks  []ContentBlock   // NEW: non-text parts (images); nil for text tools
    Details json.RawMessage  // NEW: structured, model-invisible (diff/patch/truncation/spill)
    IsError bool
}
```

`Content` stays the text channel; `Blocks` carries images; `Details` is for
renderers and compaction, never shown to the model. A text-only tool sets only
`Content`, exactly as today.

## Why additive (no upcast, no version bump)

`Blocks` and `Details` are new fields with `omitempty`. Old `ToolResultPayload`
rows (`{"CallID","Content","IsError"}`) decode into the new struct with `Blocks`
nil and `Details` empty. `CurrentEventVersion` stays 1. Confirmed against
`core/codec.go` and `core/upcast.go`.

## Changes

- Add `ContentBlock`/`ImageData` and the two `ToolResult` fields.
- `tool.NewSpec` stays (string handlers wrap to `Content`). Add a sibling
  constructor (e.g. `NewRichSpec`) whose handler returns blocks plus details, for
  read, edit, and bash.
- `ProjectEvent` maps `Result.Blocks` onto the tool message (Phase 2 adds
  `Message.Parts`); until then it carries `Content` as today.
- Write an ADR recording the content-block and details shape and the additive
  rationale.

## Dependencies

None. First foundational phase.

## Verification

Static. Build, vet, test -race, generate-check, lint. Existing tool and
projection tests stay green unchanged (proves additivity).

Runtime. Control-cli dispatches a text tool (asserts `Content` path unchanged)
and a tool returning a block plus details (asserts both round-trip through the
log and projection).
