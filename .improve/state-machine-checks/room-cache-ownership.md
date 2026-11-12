# Private room-cache preparation

A refused real websocket push could leave operations in shared Loro state. A fresh
socket resent those denied operations as history heal and was accepted as a new
stored row. The cache also exposed the denied version vector to readers. Original
actual socket checks produced1authorized control/2semantic failures; separate
in-flight-read and concurrent-preparation cases failed before product edits.

PendingRoomPush now owns a moved cache and a per-room admission lease from the
existing duplicate checks through validation, durable append and author bookkeeping.
Refusal, error or cancellation drops private speculation, leaving another reader's
newer cache alone. The sole consuming publication call follows successful
nonduplicate append. Readers rebuild durable state independently. Cold-reader and
accepted-push publication retain an already-published cache and advance against
durable rows/floor, preventing an older build from discarding newer history. No
cached mutable Loro clone escapes; ordinary warm preparation moves instead of
cloning. Existing authorization, rate/heal/opaque-row policy and duplicate fast
paths stay intact. The lease is released before refusal/ACK network sends.

Final13covered-push native tests pass (9new including32finite native generated
Loro/SQLite traces,4existing controls). Those are finite runtime checks, not an
exhaustive proof or EffectSchema/shrinking result. See CACHE_VERIFICATION.json.
The combined parent gate runs65distinct secret/vault/restart/cache tests and
all-target relayClippy with warnings denied; RELAY_COMPOSED_GATES.json records
exact source/counts/log hashes. Independent review found the per-room cache/DB
lock order and private-candidate lifecycle coherent; this is separate from the
future complete combined-source CI gate.

## Needs the owner

Review the closed denied-ops visibility/authorization gap and longer per-room
admission ownership before merge. Standalone Phase4 still invokes the historical
room_state aggregate while refreshing before ACK; final Phase7room_position removes
that retained-history scan. This correctness change has no standalone/app latency
claim. Tests use fresh synthetic SQLite fixtures and self-hosted random127.0.0.1
listeners under a verified loopback-only fence; no production/provider access.
