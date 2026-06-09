# ADR-0005 — Durable store on pure-Go SQLite (modernc.org/sqlite)

- Status: Accepted (implemented in Phase 2)
- Date: 2026-06-08

## Context

Sessions must persist as an append-only event log with full-text search over
content (the Python original uses SQLite + FTS5). The single-static-binary
distribution goal forbids a CGo dependency (CGo breaks easy cross-compilation
and clean static linking).

## Decision

Use `modernc.org/sqlite` — a pure-Go SQLite with FTS5 support. No CGo, so
cross-compilation for linux/macOS/windows/arm stays a one-machine build and the
binary stays static. The store implements the existing `ports.SessionStore`
interface, so it is a drop-in replacement for `fake.Store`.

Schema is event-sourced: a `sessions` table + an append-only `events` table
(with `Seq` unique per session) + an FTS5 virtual table over message content.
`ProviderMeta` (ADR-0003) is serialized in the event row.

## Consequences

- Single static binary preserved; FTS5 search ported with no regression.
- Pure-Go SQLite is slightly slower than the CGo build; acceptable for this
  workload (session I/O, not OLAP).
- Migrations are forward-only, versioned in code.
