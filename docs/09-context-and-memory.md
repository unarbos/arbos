# 09 — Context & memory

> Status: foundational data model + seams built; policy/backends later. See
> ADR-0014 (compression in-place) and ADR-0015 (memory & injected context).
> This is a core feature of the agent, so the shapes are locked early.

## The one idea

Context is a **projection of the append-only event log** (`core.Project`).
Compression and memory are not special storage mechanisms — they are just more
event kinds that the projection folds and renders. The log stays the single
source of truth; nothing is mutated or copied.

## What the projected conversation looks like

`Project(events, systemPrompt)` produces three regions, in order:

1. **System prompt** — stable base instructions.
2. **Injected-context block** — the latest `Segment` per source (memory, skills,
   retrieval), each fenced `<<source>> … <</source>>`. Stable prefix →
   prefix-cache friendly; fenced → injection-safe.
3. **Conversation** — user/assistant/tool turns, with compressed spans folded
   away and their summaries rendered in place.

## Compression: fold, don't rotate

```
log:  [u0][a1][u2][a3][COMPRESS lo=0 hi=1 "SUMMARY"]
                         │
Project ▼
sys, "SUMMARY", u2, a3      # span [0,1] folded; summary at the span start
```

- A `CompressionPayload{Summary, ReplacedSeqLo, ReplacedSeqHi}` is **appended**;
  the session stays `active`. No child session, no copying, no lock (contrast
  Hermes session rotation — ADR-0014).
- Raw events stay in the log → compression is **reversible** (re-project without
  folding for trajectories/audit).
- **Re-compression nests**: a later, strictly larger span subsumes an earlier
  summary; the projection renders only the outer summary. (`Project` is fuzzed to
  never panic on arbitrary/overlapping spans.)
- The trigger ("when / how much") is the single `ContextPolicy` port — not
  scattered across the loop. Summarization itself (auxiliary model) is a later
  phase; the data model and projection are done now.

## Memory: logged, fenced, latest-per-source

```
turn start →  MemoryProvider.Retrieve(query) → []Segment
            →  append ContextPayload (logged → deterministic replay)
            →  Project renders latest segment per source, fenced, as prefix
turn end   →  MemoryProvider.Observe(turn)  (best-effort write-back)
```

- **Logged, not ephemeral**: replay reproduces exactly what the model saw.
- **Latest-per-source**: the prompt doesn't grow with every recall; old
  injections are superseded (still in the log for audit).
- **Best-effort**: a retrieval/write-back failure never aborts the turn (memory
  degrades gracefully); persisting a retrieved segment uses the normal
  source-of-truth append discipline.
- **One seam** (`ports.MemoryProvider`) backs the built-in store and external
  backends alike; the engine never branches on which. Optional (nil = disabled),
  wired via `engine.WithMemory(...)`.
- **Scope is per-delegation**: whether a sub-agent sees/writes parent memory is
  part of the `Grant` (ADR-0013), replacing Hermes's ad-hoc `skip_memory`.

## Why this beats the Hermes approach

| Concern | Hermes | arbos |
|---|---|---|
| Compression | rotate session, copy rows, SQLite lock, lossy | append one event, fold in projection, reversible |
| Trigger sites | four scattered | one `ContextPolicy` seam |
| Memory in prompt | frozen snapshot mutating the system prompt | fenced block, latest-per-source, stable prefix |
| Replay fidelity | injected context not recorded | injected context logged |
| Injection safety | manual string fencing | structural fence in projection |
| Token accounting | re-estimated in the loop | `UsagePayload` in the log (ADR-0011) |

## What is built vs later

- **Built now**: `CompressionPayload` range-folding in `Project`;
  `Segment`/`ContextPayload` + logging; latest-per-source fenced rendering;
  `MemoryProvider` engine wiring (retrieve→log→render→observe);
  `ContextPolicy` port (seam declared). All `go test -race` + fuzz green.
- **Later**: `ContextPolicy` engine wiring + auxiliary-model summarization; the
  concrete memory backends (built-in store, mem0/honcho, semantic retrieval over
  FTS5); injection dedup (append context only when changed).
