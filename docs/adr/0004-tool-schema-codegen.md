# ADR-0004 — Tool JSON schemas generated from Go structs

- Status: Accepted (generator built in the tool-runtime phase)
- Date: 2026-06-08

## Context

Tool calling needs a JSON Schema per tool. In Python, Pydantic auto-derived
these from the handler's types, so schema and handler could not drift. Go has no
runtime equivalent that is both ergonomic and drift-proof. Options:

1. **Codegen** from Go arg structs at build time.
2. **Reflection** over struct tags at runtime.
3. **Hand-written** schema literals beside each handler.

## Decision

Generate schemas from Go arg structs at build time (`go:generate`). Each tool
declares a typed args struct; the generator emits the `core.ToolSchema`
`Parameters` JSON. The handler unmarshals into the same struct, so schema and
handler share one source of truth and **cannot drift**.

## Consequences

- Upfront tooling cost (a small generator) — accepted for drift-proofing.
- Schemas are static data in the binary; no runtime reflection cost at init.
- CI runs `go generate ./...` and fails if the working tree is dirty, so a stale
  schema is caught.

## Alternatives rejected

- *Runtime reflection*: drift risk between tag and handler, slower init.
- *Hand-written literals*: fine for the 2–3 skeleton built-ins but does not scale
  to the full catalog; would silently drift.
