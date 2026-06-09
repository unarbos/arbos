# ADR-0003 — Opaque provider round-trip data on messages

- Status: Superseded (2026-06-08) — the `ProviderMeta` field was removed after a
  cohesion audit. The field was plumbed end-to-end (core.Message, core.LLMChunk,
  engine accumulation, SQLite persistence) but had **zero producers**: no adapter
  ever wrote it, so the round-trip was exercised only by synthetic tests. Per
  foundational-thinking (do not carry unexercised speculative machinery), it was
  trimmed. The single real producer — the Anthropic thinking-signature multi-turn
  replay — was deferred because it cannot be implemented faithfully without a live
  key to verify the exact wire round-trip. Reintroducing the field is purely
  additive (one field on two structs + one accumulation site + persistence is
  already generic), so the seam below stands as the design to restore when a
  verifiable producer lands. The rest of this ADR is retained as that design
  record.
- Date: 2026-06-08

## Context

Some providers require opaque, provider-generated data to be sent *back* on the
next turn:

- **Anthropic** thinking mode emits a thinking-block **signature**; dropping it
  on the follow-up request causes a non-retryable 400 (the Python original has
  explicit strip-signature recovery logic to cope).
- **Gemini** carries part metadata; the **Responses API** carries item ids.

A plain `Reasoning string` on `Message` cannot carry this, and the need appears
the moment we have a second turn or persist a session.

## Decision

Add `ProviderMeta map[string]json.RawMessage` to `core.Message`. It is opaque to
the kernel: the originating adapter writes it (keyed by `LLMProvider.Name()`) on
the way out and reads its own key on the way back in. Keying by provider name
keeps sessions that switch models mid-stream unambiguous. It is persisted with
the event log so resumed sessions round-trip correctly.

## Consequences

- Native adapters (Anthropic, Gemini, Responses) can round-trip correctly
  without kernel changes.
- The event-log/SQLite schema (Phase 2) must serialize `ProviderMeta`. Decided
  now so the schema is right on first write — no migration later.
- The kernel never inspects the contents; tests assert pass-through only.
