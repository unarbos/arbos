# ADR-0022 — Multimodal message content (updates ADR-0012)

- Status: Accepted
- Date: 2026-06-08

## Context

ADR-0012 deferred multimodal content: `Capabilities.Vision` stayed false and
`core.Message.Content` was text-only. The in-house coding agent port
(docs/plans/01-pi-coding-agent) requires images: the `read` tool returns an
image, and a vision-capable model must see it. This updates ADR-0012's deferral
now that the requirement is real.

## Decision

Extend message content additively, reusing the `core.ContentBlock` shape from
ADR-0021.

- `core.Message` gains `Parts []ContentBlock` (`omitempty`). `Content` stays the
  canonical text channel. A text-only message leaves `Parts` nil and serializes
  and projects exactly as before, so no event-version bump or upcast is needed.
- `ProjectEvent` carries a tool result's `Blocks` into the projected tool
  message's `Parts`, so a tool's image output reaches the conversation.
- The OpenAI-compatible adapter serializes a message with `Parts` as a Chat
  Completions content array (a leading text part, then images as `image_url`
  `data:` URLs); a text-only message still serializes as a plain string.
- The compression token estimator counts a flat per-image proxy (4800 chars,
  matching pi) so multimodal turns are not under-counted.

`Capabilities.Vision` remains opt-in per provider/model rather than forced on:
whether a concrete model accepts images is model metadata (the model registry in
the reasoning-and-metadata phase), not an adapter-wide constant. The adapter can
now serialize images regardless; advertising vision is the registry's job.

## Consequences

- A tool result's images now round-trip through the log and into the projected
  conversation; the `read` tool can return images once its phase lands.
- Known edge: some endpoints reject image content on a tool-role message. The
  reference adapter sends it uniformly (faithful to the neutral shape); native
  adapters added later may downgrade per provider. Recorded here, decided when a
  real provider needs it.
- No store migration, no event-version bump. Existing text-only behavior is
  byte-identical.
