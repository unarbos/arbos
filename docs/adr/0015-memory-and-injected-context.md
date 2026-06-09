# ADR-0015 — Memory & injected context: logged, fenced, latest-per-source

- Status: Partially superseded (2026-06-08). The architectural pruning pass
  removed the `ports.MemoryProvider` seam, the reference keyword store
  (`internal/memory`), and the engine's retrieve/observe wiring (`WithMemory`):
  memory was off by default, wired by no entrypoint, and not part of the target
  architecture (a kernel hosting pi). What REMAINS is the injected-context event
  vocabulary — `core.ContextPayload`/`core.Segment`, `NewContextEvent`, the
  latest-per-source projection, and store persistence — kept as kernel log
  vocabulary so any future context source (RAG, manual injection) produces it
  additively without a codec change. The "fenced, latest-per-source" projection
  rules below still hold for that event kind.
- Date: 2026-06-08

## Context

Memory is a defining feature of this class of agent. The Python original baked a
frozen `MEMORY.md`/`USER.md` snapshot into the **system prompt**, needed manual
`<memory-context>` string fencing for injection safety, allowed one external
provider, and special-cased `skip_memory` for subagents to avoid prompt
poisoning. Two structural problems: (1) injected context mutated the cache-stable
system prompt, and (2) what was injected was not recorded, so a session could not
be replayed faithfully.

## Decision

Model injected context (memory recall, skill bodies, retrieved docs) as a
first-class, logged, fenced concept:

- **Logged, not ephemeral.** Retrieved context is appended to the session log as
  a `ContextPayload{Segments []Segment}` event (`Segment{Source, Content}`), so a
  replayed session reproduces exactly what the model saw. The log stays the
  single source of truth.
- **Latest-per-source rendering.** `Project` renders only the most recent segment
  per `Source`; older injections remain in the log (audit) but are superseded.
  The prompt does not grow with every turn's recall.
- **Fenced as a stable prefix.** The context block renders right after the base
  system prompt as `<<source>> … <</source>>` system messages. Stable position →
  prefix-cache friendly; explicit fence → injected text is visibly distinct from
  system instructions (injection mitigation by construction, not ad-hoc string
  scrubbing).
- **Retrieval/write-back is a port.** `ports.MemoryProvider`
  (`Retrieve`/`Observe`) is the one seam backing both the built-in store and
  external backends; the engine never branches on which. It is optional — nil
  means memory is disabled. Retrieval is best-effort (a failure means "no memory
  this turn", never a turn abort); persisting what was retrieved follows the
  normal source-of-truth append discipline.
- **Memory scope is per-delegation.** Whether a sub-agent sees/writes parent
  memory is part of the delegation `Grant` (ADR-0013), replacing Hermes's
  ad-hoc `skip_memory`.

## Consequences

- Cache-stable system prefix; deterministic replay; injection-safe rendering.
- Slight log growth from logged injections; mitigated by latest-per-source
  rendering and a future dedup (only append when changed).
- Engine wiring is complete for the retrieve→log→render→observe path against any
  `MemoryProvider`; concrete backends (built-in store, mem0/honcho, semantic
  retrieval over FTS5) are later work.

## Alternatives rejected

- *Snapshot in the system prompt* (Hermes): busts prefix cache, not replayable,
  needs manual fencing. Rejected.
- *Ephemeral injection (not logged)*: breaks deterministic replay. Rejected.
