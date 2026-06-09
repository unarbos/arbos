# Phase 3 — Reasoning level and model metadata

Back to [overview](./overview.md). Foundational (scaffold). Decisions D3 and D4.

## Goal

Preserve the model intelligence that skipping pi-ai would otherwise cost: a
per-request reasoning level, prompt-caching hints, and per-model metadata
(context window, max tokens, reasoning and vision and caching support,
thinking-level map) that drives request building, prompt caching, and the
compaction trigger.

## Current shape and the trace

`core.LLMRequest` is `{ Model; Messages; Tools; Stream; Temperature; MaxTokens }`
(`internal/core/llm.go`). It is built fresh each iteration in `engine.runTurn`
and never persisted, so adding a field is trivially additive. `engine.Config`
carries `Model`, `SystemPrompt`, `MaxIterations`. `ports.Capabilities` already
has `Reasoning` and `Vision` bools. arbos has no per-model context-window or
thinking-map data; `compaction.TokenBudget` uses a fixed `MaxTokens` today.

Pi maps a thinking level onto each request per model (`SimpleStreamOptions.reasoning`,
`Model.thinkingLevelMap`, `ThinkingBudgets`), and drives compaction from
`model.contextWindow` (`compaction.shouldCompact`).

## Target type design (sketch)

```go
// core
type ReasoningLevel string
const ( ReasoningOff, ReasoningMinimal, ReasoningLow,
        ReasoningMedium, ReasoningHigh, ReasoningXHigh ReasoningLevel = … )

type CacheRetention string // "" | "none" | "short" | "long" (pi's set)

type LLMRequest struct {
    Model          string
    Messages       []Message
    Tools          []ToolSchema
    Stream         bool
    Temperature    *float64
    MaxTokens      int
    Reasoning      ReasoningLevel // NEW: "" = provider default
    CacheRetention CacheRetention // NEW (D9): "" = provider default ("short")
    SessionID      string         // NEW (D9): stable id for session-affinity caching
}

// engine.Config gains: Reasoning ReasoningLevel, CacheRetention CacheRetention.
// The engine already owns the session id (c.id); it sets req.SessionID from it.

// internal/agent/pi (NOT core): the metadata registry
type ModelInfo struct {
    ID            string
    ContextWindow int
    MaxTokens     int
    Reasoning     bool
    Vision        bool
    Caching       bool                      // supports prompt caching / long retention
    ThinkingMap   map[ReasoningLevel]string // pi level -> provider effort value
}
type ModelRegistry map[string]ModelInfo // seeded for the models actually used
```

## Access flow

- The pi assembly (Phase 15) sets `Config.Reasoning` and the model from
  `ModelRegistry`. `engine.runTurn` copies `Config.Reasoning` onto `LLMRequest`.
- `openai` maps `Reasoning` through `ModelInfo.ThinkingMap` to `reasoning_effort`
  (or the endpoint's reasoning param), ignoring it when unsupported.
- The compaction policy (Phase 13) reads `ModelInfo.ContextWindow` for its
  trigger, replacing the fixed token budget for pi sessions.

## Prompt caching (D9)

pi drives prompt caching with a `cacheRetention` hint ("none"/"short"/"long")
and a session id (`StreamOptions.cacheRetention`, `sessionId`). The OpenAI-compat
layer maps these to `prompt_cache_retention`, Anthropic-style `cache_control`
markers on the system prompt, last tool, and last message, and session-affinity
headers (`session_id`, `x-session-affinity`). arbos already has a stable
`core.SessionID`, so the engine sets `req.SessionID` from `c.id` for free.

- The pi assembly (Phase 15) sets `Config.CacheRetention` (default "short").
- The adapter applies caching only when `ModelInfo.Caching` is true, mapping the
  retention and emitting session-affinity headers; it is a no-op otherwise.

## Why additive

`LLMRequest.Reasoning`, `CacheRetention`, `SessionID`, and the `Config` fields are
ephemeral, never persisted. The registry is a new package. No core persistence
change, no event-version bump.

## Dependencies

Lands with the foundational cluster (Phases 1 to 2) so later phases build on the
final request shape.

## Verification

Static. Build, vet, test -race, generate-check, lint.

Runtime. Control-cli runs a turn with a reasoning level set against the fake
provider and asserts it reaches the request; an unknown model falls back to
defaults without panicking.
