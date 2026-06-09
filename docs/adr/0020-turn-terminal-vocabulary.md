# ADR-0020 — Turn terminal vocabulary: error taxonomy, stop reasons, queued input, delivery envelope

- Status: Accepted (built)
- Date: 2026-06-08
- Relates to: ADR-0013 (delegation/envelope), ADR-0018 (interaction vocabulary)

## Context

An experience-first review found the kernel's internals (persistence, interrupt
safety, panic isolation) had matured while the *outbound vocabulary* every
frontend renders from — `KernelEvent` — was still v0. Concretely:

- `ErrorEvent{Err string}` collapsed provider failures, persistence failures,
  history-load failures, and kernel bugs into one opaque string. A frontend
  could not distinguish "rate-limited, retry" from "fatal", nor offer a sensible
  retry affordance.
- `TurnComplete{FinalResponse}` carried no token usage (forcing a frontend to
  re-read the log to show cost) and no stop reason. Hitting the iteration cap was
  emitted as an `ErrorEvent` — a normal guardrail rendered as a red failure.
- A prompt sent while a turn was in flight was **silently dropped** by the actor
  — the cardinal UX sin (lost user input with no feedback).
- Events carried no delivery metadata, so nested delegated-agent activity
  (ADR-0013) could not be attributed to a session or rendered as nested.

## Decision

Enrich the vocabulary and the actor, all additive:

1. **`ErrorEvent{Category ErrorKind, Retryable bool, Err string}`.** `ErrorKind`
   ∈ {provider, history, persist, internal}. Field is `Category` (not `Kind`) to
   avoid colliding with the `Kind()` discriminator method.
2. **`TurnComplete{FinalResponse, StopReason, Usage}`.** `StopReason` ∈
   {answered, max_steps}. Usage is the turn's aggregate token accounting.
3. **Max-iterations is a completion, not an error** — emitted as
   `TurnComplete{StopReason: max_steps}` with whatever work was produced.
4. **`Queued{Text}` + queue-on-busy.** A prompt arriving mid-turn is acknowledged
   with `Queued` and run FIFO after the current turn. The session actor owns the
   queue (no lock); `processPrompt` returns prompts it collected mid-turn and the
   actor drains them. Interrupts still cancel the current turn; other unexpected
   intents surface an `ErrorEvent` rather than being swallowed.
5. **`Envelope{SessionID, Depth, Event}`.** Events leave the kernel wrapped with
   delivery metadata. `Depth` is 0 for the attached session and increments per
   relay hop from a delegated child (ADR-0013), so frontends render nested
   sub-agent activity. The wrapper (vs fields on every variant) means new event
   types need no envelope boilerplate and the relay layer sets it in one place.

## Consequences

- Frontends render error states, cost, and stop reasons legibly, and never lose
  user input — the experience the vocabulary is built to serve.
- `Conversation.Events()` now yields `<-chan core.Envelope`; consumers switch on
  `env.Event`. One-time ripple through `main` and the test drains (done).
- The interaction codec (ADR-0018, Phase 7) will serialize `Envelope` with the
  nested `Event` as a tagged union; only the shape is fixed now.
- Suspend-and-await response intents (ADR-0018) arriving mid-turn currently
  surface as an `ErrorEvent`; the await-wiring that makes them first-class lands
  with the tool runtime.

## Alternatives rejected

- *Envelope as fields on every event variant* — boilerplate on each type and a
  per-variant place to forget; the wrapper centralizes it.
- *Keep max-iterations as an error* — misrepresents a normal guardrail as a
  failure and hides the partial work the user can still use.
