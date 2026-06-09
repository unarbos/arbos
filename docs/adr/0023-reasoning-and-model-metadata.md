# ADR-0023 — Reasoning level, prompt-cache hints, and a model-metadata registry

- Status: Accepted
- Date: 2026-06-08

## Context

The in-house coding agent reuses arbos's `LLMProvider` port and broker rather
than porting pi-ai (docs/plans/01-pi-coding-agent, D1/D4). That reuse would
otherwise drop three pieces of pi-ai intelligence that materially affect coding
quality and cost: a per-request reasoning/thinking effort, prompt caching, and
per-model metadata (context window, token limits, reasoning and vision support)
that pi uses to build requests and to decide when to compact.

`core.LLMRequest` is built fresh each iteration and never persisted, so adding
request fields is additive with no log-shape impact.

## Decision

Carry the request-shaped intelligence in the kernel and the per-model metadata
in the pi layer.

- `core.LLMRequest` gains `Reasoning ReasoningLevel`, `CacheRetention
  CacheRetention`, and `SessionID string` (all "" / empty = provider default).
  New `core.ReasoningLevel` (off..xhigh) and `core.CacheRetention`
  (none/short/long) mirror pi's sets.
- `engine.Config` gains `Reasoning` and `CacheRetention`; the engine sets
  `LLMRequest.SessionID` from the conversation id, so session-affinity caching is
  free for any provider that uses it.
- The OpenAI-compatible adapter maps `Reasoning` to `reasoning_effort`
  (clamping the absent "xhigh" to "high") and, when a session id is present and
  caching is not disabled, sends `session_id` / `x-session-affinity` headers.
  These headers are ignored by endpoints that do not use them, so the change is
  safe on a generic server. Body-level `cache_control` markers (Anthropic) and
  exact effort ladders belong to native adapters added later.
- A model-metadata registry (`internal/agent/pi`, NOT core) holds `ModelInfo`
  (context window, max tokens, reasoning/vision/caching support, reasoning-level
  map) with a conservative fallback for unknown ids. The compaction trigger reads
  `ContextWindow`; request building reads reasoning support and the map.

## Consequences

- Reasoning effort and prompt-cache affinity reach the provider while arbos keeps
  its own provider port; no pi-ai dependency.
- An unknown model degrades gracefully via the fallback (modest context window so
  compaction triggers early) rather than failing.
- No persistence change, no event-version bump. The registry seeds with only the
  fallback today; concrete models are added in the assembly phase.
