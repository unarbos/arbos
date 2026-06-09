# ADR-0002 — Two model ports: LLMProvider vs AgentTransport

- Status: Superseded (2026-06-08). The `AgentTransport` port was removed during
  the architectural pruning pass: it had zero implementations and no near-term
  consumer, and arbos chose to build pi as a first-class NATIVE agent on the
  completion loop (`LLMProvider`) rather than host external agent runtimes. The
  external-runtime path this port abstracted is not part of the target
  architecture. `LLMProvider` (the kept port) and the `Agent` composition layer
  (ADR-0013) carry delegation. Reintroducing a transport port is additive if an
  external-runtime backend is ever needed. The rest of this ADR is retained as
  the original design record.
- Date: 2026-06-08

## Context

Most providers are stateless completion endpoints: the kernel owns the
LLM→tool→repeat loop and calls the provider once per iteration. But some
"providers" are session-oriented subprocess agents that own their *own* loop —
Codex app-server and Copilot ACP. In the Python original these **bypass the turn
loop entirely** (`codex_app_server` skips `run_conversation`). Forcing them into
a single completion-shaped `LLMProvider` interface would break the engine's core
assumption, and we would discover it only at Phase 4+ — after building on the
wrong shape.

## Decision

Model two distinct ports:

1. **`LLMProvider`** — stateless completion. `Stream(ctx, LLMRequest) ->
   <-chan LLMChunk`. The **kernel** owns the loop. Built now; the reference
   adapter is `fake.Provider`.
2. **`AgentTransport`** (future) — owns its own loop. The engine **delegates** a
   whole turn to it and consumes the same `KernelEvent`s, persisting via the same
   `SessionStore`. Declared as an agreed shape in [03-ports](../03-ports.md); not
   implemented until its phase.

The session actor's turn step is therefore "run a turn," abstract over *who*
runs it. We do not hard-code "call `LLMProvider.Stream`" as the only way a turn
can happen.

## Consequences

- The engine is structured for delegation from day one (cheap now).
- We avoid a costly engine rewrite when Codex/ACP land.
- Slight indirection: the turn step is an interface, not a direct call. Accepted.

## Alternatives rejected

- *One port, shoehorn agent runtimes in*: breaks the loop assumption; rejected.
- *Build AgentTransport now*: violates the destination-lock scope; deferred.
