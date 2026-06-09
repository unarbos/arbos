# ADR-0019 — Session identity: reserve Principal and Origin

- Status: Accepted (fields reserved now; keying/auth semantics defined in the gateway/frontend phase)
- Date: 2026-06-08
- Relates to: ADR-0014 (ParentID is fork lineage, not identity)

## Context

`Session` currently carries only an `ID`. The gateway/frontend phase needs two
more facts about a session that don't exist yet:

- **who owns/authorizes it** (for auth and per-user policy), and
- **where it came from** (to route a reply back to the originating surface).

Hermes derived a session key from `platform + chat + user + thread`. We will need
the equivalent. `ParentID` is *not* this — post-ADR-0014 it means fork/branch
lineage, not identity.

## Decision

Reserve two metadata fields on `core.Session` now:

- `Principal string` — who owns/authorizes the session (a user or account id).
- `Origin string` — the frontend/platform plus its native addressing, e.g.
  `"cli"` or `"telegram:chat/123"`.

Both are empty for local single-user sessions today. The actual session-key
derivation, auth checks, and reply routing are **deferred to the gateway/frontend
phase**; this ADR only reserves the shape.

## Why now

Session metadata fields are a one-line addition today and a thread-through-every-
call-site migration once the gateway, store mapping, and frontends exist.
Reserving the shape protects option value without building any of that now.

## Deferred

- Session-key derivation and uniqueness rules (per-user vs shared threads).
- Auth/authorization checks against `Principal`.
- Reply routing from `Origin`.
- Persistence column mapping when the SQLite store (ADR-0005) lands.

## Alternatives rejected

- *Add later when the gateway arrives* — turns a one-line reservation into a
  migration across the store and every session constructor.
- *Overload `ParentID`/`ID` to encode origin* — conflates identity with lineage
  and addressing; rejected for the same reason ADR-0014 separated them.
