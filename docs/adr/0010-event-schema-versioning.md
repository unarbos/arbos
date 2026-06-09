# ADR-0010 — Per-event schema version and upcasting policy

- Status: Accepted
- Date: 2026-06-08

## Context

Once logs exist on disk (Phase 2 SQLite) and the `Event`/`Message`/payload shape
changes — a new kind, multimodal content, a new field — replaying an old log
requires knowing which writer produced each row. Adding a discriminator now is
one field; adding it after logs exist means un-versioned legacy rows and a
guessing migration. The claim that "a type change in the doc is a one-line edit"
is false for a persisted append-only log *unless* it is versioned.

## Decision

Every `Event` carries `Version int`, stamped from `core.CurrentEventVersion`
(currently `1`) by the constructors. `Event.Validate` rejects a zero version.

Upcasting policy:

- The log is append-only and **immutable**; we never rewrite old rows.
- On read, a row with `Version < CurrentEventVersion` is passed through a pure
  upcaster chain (`v1 -> v2 -> …`) that returns the current in-memory shape.
- New fields are added as **optional/additive** so old rows upcast trivially
  (e.g. multimodal `Parts` defaults empty — see ADR-0012); this keeps most
  evolutions non-destructive.
- A new `EventKind` needs no upcaster; older binaries ignore unknown kinds in
  `Project` (already the behavior).

## Consequences

- Old logs remain replayable across schema changes.
- The SQLite schema stores `version` per event row from day one.
- Most future changes are additive and need no upcaster; only shape changes to
  an existing payload do.
