# ADR-0018 — Interaction vocabulary: wire-ready tagged unions + suspend-and-await

- Status: Accepted (vocabulary + discriminators built now; actor await-wiring and the interaction codec are phased)
- Date: 2026-06-08
- Relates to: ADR-0008/0010 (persisted-payload discriminator + versioning), ADR-0013 (delegation), ADR-0016 (secrets)

## Context

`Intent` and `KernelEvent` are the kernel's entire I/O boundary — Intents in,
KernelEvents out. Two foundational gaps existed:

1. **Not wire-ready.** They were sealed Go interfaces with *unexported* marker
   methods (`isIntent()` / `isKernelEvent()`). That is fine for the in-process Go
   TUI, but the web dashboard, desktop, remote-arbos delegation, and the Phase-7
   control seam must serialize them as a **tagged union**. The persisted log
   already solved exactly this with `Kind()` + a codec (ADR-0008/0010); the
   interaction vocabulary had not.
2. **No suspend-and-await.** The turn loop runs to completion or cancellation; it
   cannot pause mid-turn to ask the human "approve this tool call?" or "which file
   did you mean?". The actor literally dropped non-interrupt intents while busy.
   Safe tool execution (a coding agent) needs this *before* tools exist.

Secret *input* is explicitly out of scope: secret material never flows through
the conversation; it is resolved by the broker at the request boundary
(ADR-0016).

## Decision

**(a) Wire-ready vocabulary.** `Intent` and `KernelEvent` are sealed by a `Kind()`
discriminator (`IntentKind` / `KernelEventKind`) instead of an unexported marker,
mirroring `EventPayload.Kind()`. Variant fields carry `json` tags. The shape is
fixed now; a codec mirroring `internal/core/codec.go` (encode by fields, decode by
discriminator, exhaustive switch) is the later mechanical step.

**(b) Suspend-and-await.** Add correlated request/response pairs:

| Request (KernelEvent) | Response (Intent) | Purpose |
|---|---|---|
| `ApprovalRequest{RequestID, Call, Reason}` | `ApprovalResponseIntent{RequestID, Approved, Reason}` | gate a tool call |
| `ClarifyRequest{RequestID, Question}` | `ClarifyResponseIntent{RequestID, Answer}` | free-text question |

`RequestID` (new distinct id type) correlates a response to its request. **Actor
rule:** when a turn emits a request, the session actor blocks the turn awaiting
the matching response intent; an `InterruptIntent` still cancels, and other
intents are no longer silently dropped. This replaces the skeleton's "drop
intents while busy."

## Why now (cheap now, sweep later)

Adding variants to a sealed sum and swapping the seal for a discriminator is a
one-file change today. Retrofitting a tag across every variant and every
producer/consumer — and retrofitting *suspension* into the loop after tools and
frontends assume turns are fire-and-forget — is a cross-cutting sweep. Tool
approval in particular must exist before the tool runtime (Phase 5) can produce
dangerous calls.

## Scope / deferred

- **Actor await-wiring** (blocking a turn on a response) lands with the tool
  runtime, where the first approvals are produced.
- **Interaction codec** (JSON tagged-union encode/decode) lands with the control
  seam (Phase 7); only the shape is fixed now.
- **Persisting approval outcomes** as an `EventPayload` (for audit/replay) is a
  future additive payload, not built now.

## Composition with delegation (ADR-0013)

A delegated child's `ApprovalRequest` bubbles up through the event relay to the
parent (and ultimately the human), correlated by `RequestID`; the parent routes
the `ApprovalResponseIntent` back down. One mechanism, any depth.

## Alternatives rejected

- *Keep unexported markers* — cannot serialize; forces a rewrite when the first
  out-of-process frontend lands.
- *Separate side channels for approvals* — breaks the single Intent-in /
  Event-out seam that lets every frontend attach uniformly.
