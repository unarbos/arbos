# ADR-0028 — Native provider adapters (Anthropic, Google)

- Status: Accepted
- Date: 2026-06-08

## Context

The kernel shipped one real provider (OpenAI-compatible). pi targets many; the
two most important for coding beyond OpenAI are Anthropic (Messages API) and
Google (Gemini generateContent). Each has a distinct wire shape and SSE protocol.

## Decision

Add `internal/provider/anthropic` and `internal/provider/google`, each a native
`ports.LLMProvider` behind the existing port — no engine change. They mirror the
OpenAI adapter's structure: an `Authorizer` (the `secret.Broker`, configured by
the host with the right header injector — `x-api-key` for Anthropic,
`x-goog-api-key` for Google), a synchronous request with errors surfaced
immediately, and a goroutine that parses the provider SSE into neutral
`core.LLMChunk`s. Reasoning maps to each provider's thinking control (Anthropic
`thinking.budget_tokens`, Gemini `thinkingConfig.thinkingBudget`); system prompts
hoist to the provider's native slot; tool calls accumulate and emit fully formed.
A seeded `pi.SeededModelRegistry` carries context-window / token / capability
metadata for common models across all three providers.

## Consequences

- pi can run on Anthropic and Gemini through arbos's own provider port; the model
  registry drives accurate compaction and capability decisions.
- Live-credential gap: no Anthropic/Google key was available in this environment
  (checked via doppler), so the adapters are proven against faithful
  provider-shaped httptest SSE fakes (request shaping + text/tool/usage parsing)
  rather than a live call. Documented, not a blocker.
- Deferred refinements (documented in each adapter): image content parts on these
  providers, the thinking-signature round-trip (ADR-0003) on replay, and OAuth
  login flows (the broker abstracts credential injection, so OAuth is a
  token-acquisition flow to add, not an adapter change).
