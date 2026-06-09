# Phase 22 — Native providers (Anthropic, Google) + OAuth

Back to [overview](./overview.md). Status: done (against fakes; live-key gap).
ADR-0028.

## Changes (as implemented)

- `internal/provider/anthropic`: native `ports.LLMProvider` for the Messages API
  (system hoisting, tool_use/tool_result blocks, thinking budget, message_delta
  usage), auth via the broker's `x-api-key` injector.
- `internal/provider/google`: native `ports.LLMProvider` for Gemini
  generateContent (contents/parts, functionCall/functionResponse, systemInstruction,
  thinkingConfig, usageMetadata), auth via `x-goog-api-key`.
- `pi.SeededModelRegistry`: context-window / token / reasoning / vision / caching
  metadata for common Anthropic, Google, and OpenAI models.

## Verification

Static gate green, no new lints. Runtime: each adapter driven against a faithful
provider-shaped httptest SSE fake — request shaping (system hoisted, tools sent)
and response parsing (text + tool call + usage) asserted for both.

## Live-credential gap

No Anthropic or Google API key was available via doppler or env in this
environment, so the adapters were proven against fakes, not a live call. OAuth
login flows are deferred pending client credentials; the secret broker already
abstracts credential injection, so OAuth is a token-acquisition addition, not an
adapter change.
