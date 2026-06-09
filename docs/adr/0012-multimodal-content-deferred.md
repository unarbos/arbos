# ADR-0012 — Multimodal message content: decision deferred, made additive

- Status: Accepted (defer modeling; revisit when the first vision provider lands)
- Date: 2026-06-08

## Context

`Capabilities.Vision` exists but `Message.Content` is a bare `string`, so there
is no shape to carry image/audio parts — a latent contradiction (a capability
flag for something the data model cannot express). The reviewer proposed
modeling `Content` as `[]ContentPart` now to avoid a later breaking change to
the most central, persisted type.

## Decision

**Defer the type change; record the plan.** With per-event schema versioning in
place (ADR-0010), adding multimodal later is an **additive** change, not a
destructive log migration:

- a future `Message.Parts []ContentPart` is an optional field; `Content string`
  remains the text shorthand;
- v1 (text-only) rows upcast trivially with `Parts` empty.

Modeling `[]ContentPart` now would complicate every text path and the projection
before a single vision provider exists — premature structure that closes no door
(versioning keeps the door open). Honesty rule in the meantime: a provider
adapter must not advertise `Capabilities.Vision: true` until the data model can
carry parts (the fake does not).

## Consequences

- No gold-plating of the central type before there is a consumer.
- When vision lands: add `ContentPart` + `Parts`, bump `CurrentEventVersion`,
  add a (trivial) upcaster, and flip the capability — all additive.
- Revisit this ADR at that point and supersede if the additive assumption breaks.
