# Matrix migration — surface-area inventory (pre-implementation)

> Purpose: a complete, file-level map of the current codebase against
> [ADR-0041](../../adr/0041-matrix-room-is-the-session.md), so implementation can
> **cut every old surface, change every affected one, and add the new ones with
> no lingering dead code**. Compiled from a 7-agent codebase sweep (≈305 `.go` +
> `.ts/.tsx` files). Dispositions: **DELETE / REPLACE / AMEND / KEEP / OPEN**,
> plus a **NEW** set for code that does not exist yet.

## Headline

The cognition core is preserved exactly as ADR-0041 promises; **all subtraction
and replacement lives in the storage, door, identity-transport, and frontend-seam
layers.** Concretely:

| Disposition | ~Count | Where it concentrates |
|---|---:|---|
| **KEEP** | ~190 | `core` (payloads/projection logic), `engine` loop, `provider`, `tool`/`codingspec`, `mcp`, `plan`, `mind`, `head` (billing), `secret`, `fake`, most `web/src` |
| **AMEND** | ~90 | `core` (Audience/wire/identity fields), `engine` (Audience append, subtract Queued/Origin), `piwire` (wire Dendrite+mautrix), `forest` (federation), `sessiontree` (fork→room/branch→thread), `setmodel`/`mind`/`recall`/`trajectory`, `cmd/*`, `web/src` (ChatTab/ChatView/types/api) |
| **DELETE** | ~22 | `internal/messenger` (6), `internal/share` (1), 8 `gateway` files, `sqlite/grants.go` + `sqlite/outboxstore.go`, `envfile`, frontend `PeopleSurface`/`PeopleChat`/`ShareChat`/`ShareDialog` |
| **REPLACE** | ~3 | `sqlite/store.go` (event-log half → Dendrite+overlay), `web/src/lib/seam.ts` (→ mautrix client), `internal/outbox` (→ Matrix notices) |
| **OPEN** | ~10 | gaps the ADR doesn't fully settle — must decide before coding (see §OPEN) |
| **NEW** | ~6 | code that doesn't exist yet (see §NEW) |

**No file in the cognition core (`core` payloads, `engine` loop, `provider`,
`tool`, `mcp`, `plan`, `mind`, `head`, `secret`, `fake`) is DELETEd.** Subtraction
never touches how the agent thinks.

---

## Core-type collapses (ADR-0041 D15) — subtract on the data shapes first

A foundational pass found core types the substrate makes redundant. These are
**P0 work** (data shapes before logic), and they shrink the surface every later
phase touches:

| Collapse | From → to | Confidence | Touch |
|---|---|---|---|
| Identity | `Session.Principal` + `Session.Origin` → one Matrix `sender` (`@user:server`) | high | `core/session.go`, `core/message.go`, `engine/projection_internal_test.go`; **delete `Origin`** everywhere |
| Lineage | `ParentID` + `Owner` + `SpawnedBy` → Matrix relations (forked_from / thread parent room / `life.arbos.spawned_by`), derivable from the scope id | medium — verify call sites | `core/session.go`; readers in `engine`, `plan/scheduler`, `agent`, `recall`, `piwire` re-read Matrix relations |
| Control + caps | 4 `Set*Intent` → one `ControlIntent{op,value}`; `Session.{WebSearch,WebFetch,ImageGen}` + same 3 `ConfigPayload` fields → one capability set | medium (wire already unified, D10) | `core/intent.go`, `core/session.go`, `core/event.go`, `engine`, `setmodel`, `settings` |
| Suspend-await | `ApprovalRequest`/`Response` + `QuestionRequest`/`Response` → one Request/Answer pair (approval = gated yes/no question) | high (wire already unified, D10) | `core/kernelevent.go`, `core/intent.go`, `engine/ask.go`, `engine/dispatch.go`, `ask` |
| Codecs | `core/codec.go` → Local-overlay only; `core/interaction.go` → local stream only (Room events are Matrix events) | high | both shrink; adapter owns Room serialization |

**Dangling — delete (confirmed by D15):** `Queued` + prompt/message `Origin`;
sqlite `boards`/`grants`/`outbox`; `internal/envfile`; forest anonymous `acct_…`
(machine identity = the device key, D3).

**Stale docs to refresh (dangling contracts):** **ADR-0019** (Principal/Origin
reservation — consumed by D15a) and **`docs/02-data-model.md`** (must add
`Audience`/`sender`, drop the removed fields) — otherwise the canonical
data-model doc drifts from the kernel.

## DELETE set — old code to cut (by phase)

Cut these entirely; their capability returns via Matrix primitives.

### P2 — share/People machinery → membership + invite + inline `chat_note`
- `internal/share/share.go` — share-grant capability model → power levels + invite.
- `internal/sqlite/grants.go` — bearer-token grant table (+ the `grants` and vestigial `boards` tables in `migrate()`).
- `internal/gateway/share.go`, `share_scope.go` — `/s/<token>`, guest-name form, `filterShareFrame`/`stampAuthor`/`sanitizeGuestName` → homeserver-stamped `sender` (D4).
- `internal/gateway/ws.go` — `/api/ws` chat control-seam bridge → Matrix sync + local delta stream.
- `internal/gateway/presence_test.go`, `chatnote_test.go`, `share_scope_test.go` — test the deleted roster/typing/chat_note/share-scope seam.
- `web/src/ShareChat.tsx`, `web/src/components/ShareDialog.tsx`, `components/PeopleSurface.tsx`, `components/PeopleChat.tsx` — → inline side-notes + member list + invite UI.

### P3 — control-seam remote codec → federation + to-device
- `internal/control/server.go` — **partial DELETE**: the remote-agent NDJSON wire and fork/branch/echo-suppression frames go; an OPEN question is whether a slim local stdio/TUI adapter survives (see §OPEN).
- `internal/control/server_test.go` — remote-wire tests.

### P4 — Telegram door → mautrix bridge sidecar
- `internal/messenger/{service,telegram,ask,format,stream}.go` + `ask_test.go` — the entire hand-rolled Telegram bridge.
- `internal/gateway/messenger.go` — its REST/WS face.
- `web/src/components/MessengerView.tsx`, `web/src/lib/messenger.ts` — its UI (OPEN→DELETE when the bridge lands).

### Independent of phase (golden-path cleanup)
- `internal/envfile/envfile.go` — plaintext `.env` loader; forbidden by ADR-0016/0033 (secret vault only). Remove its sole caller in `cmd/arbos/main.go`.
- `internal/sqlite/outboxstore.go` + `internal/outbox/outbox.go` — see REPLACE (the SQLite outbox table + claim-then-deliver pattern is cut; semantics move to Matrix).

---

## REPLACE set — gut and reimplement on Matrix

- **`internal/sqlite/store.go` (event-log half)** — the `events`/`sessions`/`events_fts` tables as the production `SessionStore` are replaced by **embedded Dendrite (Room-audience) + a small `.arbos/` local-overlay table (Local-audience)**. The brain tables (`atoms`, `plan_nodes`, `mind_checkpoints`) **KEEP** in the same DB. `ListSessions`/`Search`/`RecentSessions` reads re-target to room timeline + overlay (Spaces for the tree, D11).
- **`web/src/lib/seam.ts`** — the WebSocket control-seam client becomes a **mautrix-go-style Matrix client**: room sync (durable timeline), `/sendToDevice` (control), and a separate local attachment for streaming deltas (D6/D10). Every `SeamClient` importer (`ChatTab.tsx`) rewires through it.
- **`internal/outbox`** — between-turn "voice when no door is open" → Matrix room events (`m.room.message`/`m.notice`) + push/unread, not SQLite claim-then-deliver. (Marked OPEN by the persistence agent; decide closed-conversation push = to-device vs notice.)

Also: the **`SessionStore` *reference implementation*** is REPLACE (D2), while the
**`ports.SessionStore` interface** itself is KEEP (+ doc'd semantics) and
`fake.Store` is KEEP for tests.

---

## AMEND set — in-place changes (the load-bearing ones)

**Kernel / data model**
- `core/event.go` — **add `Audience` field** (D1); map each `EventKind` to a default `AudienceLocal`/`AudienceRoom`.
- `core/project.go` — `ProjectEvent`'s `ok` means *model projection only*; stop conflating with federation (D1). chat_note exclusion becomes one cell of the Audience×projection 2×2.
- `core/intent.go`, `kernelevent.go`, `interaction.go` — route content→room, control→to-device, presentation→local (D10); **delete `Queued`**; **drop `Origin`** from prompt/chat-note + `Message` (self-suppress via txn-id + `sender`).
- `core/session.go`, `id.go`, `scheduler.go`, `upcast.go` — `Principal`/`Origin`/`Author` = Matrix `sender`/homeserver (D4); `SessionID` documented as projection scope room|room+thread (D11); `Audience` additive upcast (P0).
- `core/message.go` — `Author` = `sender`; remove `Origin`.
- `engine/engine.go`, `compress.go`, `registry.go`, `tree.go` — set `Audience` on append; consume folded `Events`; subtract Queued/Origin paths; `sender→Grant` at the actor edge (D12); addressed-only activation (D13); fork→room / branch→thread (D11); compaction summaries stay `Local`.
- `ports/ports.go` (+ `porttest/store.go`) — `SessionStore` semantics: Audience routing, merged `Events`, room-ordering `Seq`.

**Persistence / trajectory**
- `sqlite/plannodes.go` (`LastHumanSeen`), `sqlite/atoms.go` (checkpoint cursor) — follow the merged room+overlay timeline / new seq semantics.
- `trajectory/trajectory.go`, `recall/tool.go`, `mind/mind.go`+`curate.go` — read **room raw + per-agent Local overlay** (per-agent trajectory; D1 consequence).

**Identity / transport**
- `forest/identity.go` — add HKDF `derive(device,"matrix/federation")` + `derive(device,"matrix/user/<id>")`; device-only digest → machine `server_name` (D3/D8).
- `forest/client.go`, `head.go`, `wire.go` — two leases per machine (HS + per-dir web entry); relay carries federation; lease = URL liveness not delivery (D8/#9).
- `secret` — **add** the outbound redaction twin (see NEW); inbound broker KEEP.

**Agent / sessions**
- `sessiontree/sessiontree.go` — Fork → new room + `life.arbos.imported` re-emission; Branch/Accept/Discard → thread-scoped projection + parent-room summary inject (D11/ADR-0027/0037).
- `agent/{arbos,delegate}.go`, `agent/pi/agent.go` — child spawn → Matrix invite + thread/child room; `Grant` also the per-member token (D12); `Budget` extends to room budget + hop counter (D13).
- `setmodel/setmodel.go` — `SetModelIntent` → to-device `life.arbos.control` (D10).

**Wiring / entrypoints / frontend**
- `piwire/wire.go`, `config.go` — **the primary wiring AMEND**: assemble embedded Dendrite + mautrix `SessionStore`; `NewSessionID`→room ids; shrink account resolve to LLM endpoint + vault.
- `cmd/arbos/{main,cli,web,tui,render,export,...}.go` — embed Dendrite on loopback; `arbos .`→web entry (D8); TUI becomes a Matrix client; export reads room+overlay.
- `cmd/forest`, `cmd/head` — head shrinks to DNS/TLS/relay + money/secrets.
- `web/src`: `ChatTab.tsx`/`ChatView.tsx` (unified room-timeline renderer: inline chat_notes, branch/sub-agent threads, attribution); `lib/types.ts`/`transcript.ts`/`api.ts`/`identity.ts`/`workspace.ts`/`settings.ts` (drop roster/typing/chat_note/share frames, `peopleFromReplay`, `dedupePeople`, `linkPeopleChat`, `shareArtifact`); `TabStrip.tsx`/`App.tsx`/`SettingsView.tsx` (remove People/Share affordances); `ApprovalCard`/`QuestionCard`/`ModelPicker` (→ room/to-device events).

---

## KEEP set — the cognition core (do not touch)

`core` payload codec + `Message`/`ToolCall`/`Usage`/`Atom`/`Envelope`/`AccessSet`/
`estimate`; `engine` dispatch/stream/relay/ask/toolsink; `ports` LLMProvider/
ToolRuntime/Clock/Summarizer/ContextPolicy/ApprovalPolicy/Observer/SecretProvider;
`compaction`; `provider/*` (all adapters — `provider_meta` just routes to Local);
`tool`/`codingspec`/`jsonschema` (minus the denylist gap below); `mcp`; `browser`;
`ask`; `modelcatalog`; `theme`; `obs`; `extension`; `fake/*`; `head/*` (billing
ledger/gateway/EVM); `secret/*` (inbound broker); `plan/*`; `mind` tool; `transcript`;
and the bulk of `web/src` (surfaces, markdown, charts, terminal, jobs, settings panels).

---

## NEW set — code to add (doesn't exist today)

1. **The Matrix adapter** (`internal/matrix/` or similar) — embeds Dendrite on loopback, runs the mautrix-go client, translates `event.Event ⇄ core.Event` and `core.Intent → send/to-device`. **Must never be imported by `internal/core`.**
2. **The Local-overlay store** — the `.arbos/` table for `AudienceLocal` events (reasoning/usage/config/context/compaction/`provider_meta`), folded with the room timeline by `Events()`.
3. **`core.Audience`** — the enum + field (P0 keystone).
4. **The `.arbos/` tool denylist** — ADR-0035/D3 mandates blocking `.arbos/` and `~/.config/arbos/` in file/shell tools; **verified absent today** (`tool/path.go` + `codingspec/{ls,read,write,edit,bash,find,grep}.go`).
5. **The outbound redaction/exposure twin** (D12) — at the Local→Room raise: block/redact/owner-approve secrets (`.env`/`.pem`/high-entropy/known `SecretRef`s). Likely `internal/matrix/exposure.go` or `internal/secret/redact.go`.
6. **The node-private-state backup** (#12/D12) — encrypted backup of the Local overlay + E2EE keys + `provider_meta` to the always-on home server, unlocked by D9's recovery key. Plus the **invite UI** replacing `ShareDialog`.

---

## OPEN set — RESOLVED

> All ten are now settled in
> [ADR-0041 §"Resolved operational questions"](../../adr/0041-matrix-room-is-the-session.md)
> and **D14 (SSSS as the private-state substrate)**. They fold into four
> Matrix-native moves: **(A)** content/sharing → room events + extensions +
> invite/media; **(B)** private synced state → SSSS; **(C)** local replica +
> router ⟂ relay; **(D)** the in-process kernel seam + reused `Agent`/`Grant`/
> identity tokens. The detail below is retained as the gloss.
>
> A later semantics-collapse pass surfaced four design angles (not code-cut gaps),
> all now **resolved** in [ADR-0041](../../adr/0041-matrix-room-is-the-session.md):
> *turn granularity* and *room growth vs trajectory* (D1 refinement — room = lean
> shared window, local store = durable trajectory); *bootstrapping* (§"Bootstrapping:
> `arbos .` just works" — deterministic/idempotent/lazy first-run, no login); and
> *budget* (D13 — state for the cap, append-only `life.arbos.spend` ticks for the
> tally, gauge invisible until a shared working room). No design angles remain open.

1. **Outbox semantics** — closed-conversation "agent's voice" delivery: Matrix `m.notice` in the room vs to-device push? (REPLACE direction is clear; mechanism isn't.)
2. **Room/full-text search** — `events_fts` for room content: Dendrite/Matrix search vs a separate arbos index over the overlay? (overlay prose is straightforward; federated room search is OPEN.)
3. **Central secret vault** — Topology/#11 places a vault on the always-on backend, but secrets live per-node in `internal/secret/store.go` today; sync/replication strategy unset.
4. **`show`/`ui` tools** (`codingspec/{show,ui}.go`) — the `ToolResult.Details`→frontend-panel coupling; Element/Matrix clients won't consume it. Keep for the per-dir web entry client (D8) or change transport?
5. **Control-seam local survival** — does stdio/TUI keep a slim local seam, or become a full Matrix client? (`control/server.go` partial-DELETE boundary.)
6. **`TransportAgent`** (Codex/Claude/Cursor) — capture at the relay boundary is described but the Go type doesn't exist; design the adapter.
7. **D13 room budget vs scheduler** — does the plan scheduler respect a room-level `life.arbos.budget` + hop counter, or only per-wake `Grant.Budget`?
8. **Machine identity ↔ billing account link** — forest `deviceRec.AccountID` (→ machine/HS identity) vs `head.Account.ID` (billing); P2-linking/D9 person attach is unspecified.
9. **Default endpoint target** — once Dendrite is embedded, does the web/TUI client default to loopback HS or the always-on `arbos.life` head? Not encoded in `settings`.
10. **`sharedoc.go`/`trajectory.go` (gateway)** — markdown render + trajectory export currently ride share links; keep for authenticated `/api/file` panels / owner export, or fold into Matrix?

---

## Phase-ordered implementation checklist

- **P0** — add `core.Audience` (+ default map, additive upcast); document `SessionID` as scope; the HKDF identity-root derivation (`forest/identity.go`). Pure data-structure work; nothing deleted yet.
- **P1** — NEW Matrix adapter + embedded Dendrite (loopback, one per machine) + Local-overlay store; REPLACE `sqlite/store.go` event half and `seam.ts`; AMEND `piwire`/`cmd/arbos` wiring, `ChatTab`/`ChatView`. Every session is a room on your own homeserver. (Deletes the dual store.)
- **P2** — DELETE the share/People set (above) + AMEND the gateway/frontend that referenced it; membership + power levels + `sender→Grant` (D12) + the `.arbos/` denylist (NEW #4) + the outbound redaction twin (NEW #5).
- **P3** — federation over the relay; `server_name`⟂web-entry split; remote delegation as federated events; DELETE the control-seam remote codec.
- **P4** — person identity on the commons + cross-signing/key-backup + node-private-state backup (NEW #6); DELETE `internal/messenger` + adopt a mautrix bridge; shrink the forest head.
- **P5** — multi-agent working rooms (D13); wallet/attestation events; agent-to-agent payments.

> Verification gate per phase (per arbos convention): `go build/vet/test -race`
> green + the skeleton runs. The DELETE set for a phase must be fully removed
> before that phase is considered done — no stubs, no dead imports (the doors
> agent listed the exact import edges to cut: `gateway → share/messenger/control`).
