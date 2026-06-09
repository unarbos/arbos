# ADR-0011 — Token usage and reasoning have a home in the log

- Status: Accepted
- Date: 2026-06-08

## Context

`Usage` (on `LLMChunk`) and `Message.Reasoning` were captured but dropped: the
chunk loop never read `ch.Usage`, nothing wrote `Session.TokenCount`, and the
persisted assistant event omitted reasoning. For a system that wants audit and
trajectory generation, both were structurally homeless — the type existed but
the concern was wired nowhere.

## Decision

**Usage** is persisted as a first-class `UsagePayload` event after each LLM
response, so it lives in the log and in any trajectory derived from the log.
`Session.TokenCount` is a **derived mirror** updated best-effort via
`UpdateSession` (non-authoritative; the usage events are the source of truth, so
a failed mirror write does not abort the turn — see ADR-0009).

**Reasoning** is accumulated in `streamResponse` and persisted on the assistant
`MessagePayload` (`Message.Reasoning`). Combined with `Message.ProviderMeta`
(ADR-0003), this lets reasoning-mode providers round-trip thinking content and
its signature, and lets replay/trajectories reconstruct reasoning.

## Consequences

- No dead fields: every captured datum has exactly one persisted home.
- Trajectories include usage and reasoning.
- Token accounting can be recomputed from the log if the mirror is ever stale.
