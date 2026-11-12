# Phase 1: system map

Base: `2865cb29b`. Scope: the whole repository, with backend, native clients and
auxiliary runtime reviewed separately. This is a navigation map, not a claim
that each of roughly 513,000 source lines has been individually audited.

## Main flows and stores

| Flow | Code and state | External edges / critical work |
| --- | --- | --- |
| Open a project or people/group chat | GPUI `arbos-mac/src/sync/relay.rs`; Swift `Store/AppModel.swift`, `Sync/SessionRoom.swift`; shared `arbos-client/src/room.rs` | registry/ACL reads, local snapshot, room handshake, checkpoint/row import, transcript projection, first render |
| Send a message | native composer → local Loro document → room PUSH → relay `rooms.rs` → SQLite append/ACK/broadcast | optimistic display and durable delivery are different milestones; keep per-room pending batches and cursor/snapshot persistence |
| Agent reply / background job | relay wake → `arbos-agentd` workspace, `agent.rs`, hooks and jobs → pi extension `harness/arbos-pi-ext/index.ts` | route-first acknowledgement, approval, memory/working set, provider inference, subagents; tool/process lifetime outlives HTTP requests |
| Servers / channels | app-only `arbos-matrix` → relay Matrix proxy and outbox → homeserver | sliding sync, room/member metadata, sender-name resolution, channel timelines; migration flags select paths |
| Voice / video | relay calls and voice endpoints → Python `voice/arbos_voice/worker.py`, `updates.py`, `vision.py` | LiveKit media, OpenAI realtime/STT, relevance model, relay cost reports; one speaker per call |
| Files / published sites | relay blobs/sites → R2; `edge/sites-worker/src/index.js` reads manifests and content-addressed blobs | separate CDN cache/manifest lifecycle; Rust and JS duplicate routing rules |
| Local/outside tools and updates | `arbos-local`, `arbos-acp`, `arbos-sidecar`, `arbosctl`, `arbos-update` | approval/signature/ownership boundaries; never simplify them away to reduce latency |
| Release / feedback / evaluation | `.github/workflows`, `scripts`, `image`, `evals`, static `site` | self-hosted/native builds and deployment automation; audit only, no execution of live operational flows |

The relay owns one SQLite connection behind `Arc<Mutex<Connection>>` (`db.rs:27`),
plus blob/checkpoint files and in-memory room caches. Agentd has per-workspace
SQLite history/memory/FTS and durable room state (`store.rs`). Devices have local
replicas and persistent room cursors; Matrix has its own homeserver store. Core
DTOs/wire/Loro rules are shared; Swift DTOs are generated into `packages/ArbosFFI`.
The relay/agentd must not depend on app-only `arbos-client` or `arbos-matrix`.

## Where latency can accumulate

Mac registry refresh serializes project detail/member work before chat registry
publication (`sync/relay.rs:2585`). Snapshot projection holds the shared docs
mutex while decoding transcripts (`:3014`), competing with import/send work.
Relay `chats_of_member` does a membership query per chat (`db_people.rs:483`);
`rooms::advance_room_doc` requests full `room_state` statistics even though its
cache key uses only head/floor (`rooms.rs:62`, `db.rs:1846`). Matrix sender-name
and joined-room snapshots include serial async work (`arbos-matrix/src/client.rs`).
These are code-supported candidates, not measured explanations of user reports.
Cold open, warm open, local send and provider response need separate timings.

## Design questions

1. **Abstract thinking:** treat perceived speed as a critical path with shared
   locks and durable-delivery invariants. Measure first render, local echo, ACK
   and agent completion separately.
2. **Irreducible constraints:** preserve ACLs, per-room deduplication, pending
   sends, cursor/snapshot atomicity and Loro compatibility. Bound independent work;
   do not remove necessary consistency checks.
3. **Principal architect's first change:** separate cheap room-position reads
   from statistics and pure presentation from runtime initialization. A framework
   rewrite has no supporting evidence.
4. **Day-one breaking test:** two rooms have pending sends; ACK room A, reconnect
   B, and prove B's batch still resends. The Mac adapter currently drains a global
   pending set on ACK; iOS uses room-scoped sets. A runnable GPUI fixture is blocked
   by baseline build space, so this cross-room test is deferred rather than
   replaced by a test of a disconnected model.
5. **Higher abstraction:** room-scoped session ownership and immutable
   presentation snapshots. Build on the existing sans-IO `RoomSession` seam.

## Before / after and limits

Runtime behavior is unchanged. Phase 0 lint/typecheck/test/build results apply
unchanged; no gates were rerun for this prose-only map. The map covers 12 Rust
crates, native Swift, harness, voice, edge/site, image, scripts and evals.
Deeper findings belong to subsequent phase reports. Native build/test blockers
remain; no production, staging, provider, Matrix or cloud services were queried.
