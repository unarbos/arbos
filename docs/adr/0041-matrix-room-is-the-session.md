# ADR-0041 — The room is the session: Matrix as the federated substrate

- Status: Proposed (sketch)
- Date: 2026-06-26
- Amends: ADR-0033 (hosted backend + device identity), ADR-0034 (gateway auth,
  tunnel, the forest), ADR-0035 (agent = (dir, machine), derived URLs),
  ADR-0036 (agent wallet), ADR-0013/0038 (delegation; multi-party + side notes)

## Context

Re-reading the kernel from scratch, a pattern is impossible to miss: arbos has
independently reinvented most of Matrix's primitives. A session is an
append-only, replayable, attributed event log (`core.Event`, ADR-0008/0010). A
message carries a server-stamped `Author` the client cannot forge (ADR-0038).
`Session.Principal`/`Origin` are *reserved* fields waiting for "who said this /
where from" (ADR-0019). "One log, many doors" (a web tab + a Telegram bridge on
one session) is the messenger's whole model. Share grants gate who may write
(ADR-0034). The forest is a node directory plus an outbound relay (ADR-0034).
The control seam is *deliberately* "the same wire for remote agents" (ADR-0013).

Those are, one-for-one: a room timeline; `sender`; reserved `sender`/origin
server; multi-client rooms; power levels; server discovery + federation; and
federated custom events. We did not set out to rebuild Matrix — we kept arriving
at its shapes because they are the right shapes for durable, attributed,
multi-party, replayable conversation.

So this ADR does **not** propose grafting Matrix onto arbos. It proposes
recognizing that **arbos's hexagonal boundary was already drawn for this
substrate** and finishing the job: make the session *be* a room, collapse five
bespoke subsystems (the SQLite-only store, the share mechanism, the forest
protocol, the control-seam codec, the per-platform doors) into one Matrix
adapter, and keep the cognition core — the turn loop, the ports, `Grant`/
`Budget`, projection — untouched.

The expensive thing to get wrong here is the **data shape**, so per foundational
thinking the Decision leads with types, not plumbing.

## Decision

> **The room is the session. The kernel projects a room into a turn and writes
> its room-visible output back as events. Identity, attribution, multi-party,
> permissions, directory, federation, delegation transport, and attestation are
> one substrate — a Matrix homeserver — and the cognition core does not change.**

### D1 (data structure, the keystone) — split the conflated axis: `Audience` ⟂ projection

Today `ProjectEvent` (core/project.go) returns `ok=false` for
`usage|compressed|context|interrupted|config|chat_note|branch_anchor`, lumping
together two genuinely different questions under one boolean because there has
only ever been one local store:

1. **Model projection** — does *this agent's own model* see the event?
   (`chat_note` is excluded because it is human-to-human.)
2. **Audience** — does the event leave this agent at all, i.e. is it federated
   to other room members/servers? (Never asked before: nothing federated.)

`chat_note` proves they are orthogonal: it is *not* model-visible but *is*
people-visible (it already broadcasts to every door). Usage is neither. A shared
tool result is both. From scratch you would name the second axis as a field on
the event, not infer it. So:

```go
// Audience is who an event federates to, independent of whether the agent's
// own model projects it (ProjectEvent). Additive (ADR-0010): the zero value is
// AudienceLocal, so every event arbos writes today keeps today's behavior —
// nothing leaves the box — and old logs decode unchanged.
type Audience uint8

const (
    AudienceLocal Audience = iota // private trajectory: never federated
    AudienceRoom                  // federated to all room members/servers
    // AudienceParticipant (a private sub-conversation) is deliberately omitted
    // now — reserved as option value, built when a DM/whisper case demands it.
)

type Event struct {
    // ... existing fields (ID, SessionID, Seq, TurnID, Version, CreatedAt) ...
    Audience Audience // NEW; zero = Local = today's behavior
    Payload  EventPayload
}
```

Mapping (the default policy; the engine may raise a tool result to `Room` in a
collaborative working room):

| EventKind | model-projected | `Audience` |
|---|---|---|
| user / assistant message | yes | **Room** |
| tool_result | yes | **Local** by default, **Room** in a shared working room |
| chat_note | no | **Room** (reaches people, not the model — ADR-0038, verbatim) |
| usage, config, context, interrupted, branch_anchor | no | **Local** |
| compressed | no (folds) | **Local** |

This single field is the foundation every later phase needs, which is why it is
Phase 0. It also resolves the hardest objection to "the room is the log":

- **Compaction stays local.** A `CompressionPayload` is `AudienceLocal`: each
  agent folds *its own projection* of the shared room. The room timeline is
  immutable shared history; an agent's summary is a private overlay on top of
  it. Compaction never mutates, never federates, never races another member.
- **Private trajectory stays private.** Reasoning detail, token usage, injected
  memory/skills/plan context (`ContextPayload`) are `AudienceLocal`: other
  agents and the homeserver never see them. The model's inner monologue is not
  federation traffic.

The model a member sees, then, is: **shared room events (`Room`) interleaved
with that member's own local overlay (`Local`)**, folded by the existing
`Project` in timeline order. `chat_note`'s projection-exclusion stops being a
special case and becomes one cell of a 2×2.

### D2 — `SessionID` is a room id; the homeserver is a `SessionStore`

`core.SessionID` is already an opaque distinct type, not a bare string, so it
holds `!roomid:server` with **zero kernel change** — the option value was
banked years early. `SessionStore` (ports.go) gains a second reference adapter
beside SQLite: a Matrix-backed store. Concretely (see Implementation) its
`Room`-audience reads come from a **gomuks/hicli local SQLite room cache** and
its writes go out through hicli/mautrix-go, while `Local`-audience events stay
in arbos's own small table keyed by room id. So `Events` folds two stores —
hicli's shared room timeline and arbos's private overlay — and `AppendEvent`
routes by `Audience`. `Seq` semantics shift from store-assigned monotonic to the
room's timeline ordering; `Project` already tolerates reordering and unknown
payloads (ADR-0010), so this is contained.

**Concurrency corollary (foundational thinking).** The actor model holds
*tighter*, not looser: each agent's session actor is the **sole writer of its
own events** (its `Local` overlay and the `Room` events it authors). The room is
multi-writer *across* agents/servers, and that merge is exactly what Matrix's
DAG + state resolution exist to do — arbos does not invent a second
reconciliation mechanism. "No shared mutable session state" (05-concurrency)
survives because no two actors write the same event; they write into a shared
*log* that converges by construction.

### D3 — one ed25519 key is the *root of trust*, not every operational key

ADR-0033 (device key), ADR-0035 (it signs `AgentID` claims), and ADR-0036
(receive/attestation) already use *one curve for one machine identity*. This ADR
extends that: the device ed25519 key is the **root of trust that authorizes the
agent's Matrix existence** — it mints (via the appservice challenge, ADR-0033)
the access token, and can cross-sign the agent's Matrix device. `AgentID =
H(devicePub‖dirPub)` (ADR-0035) derives the Matrix localpart and the room/space
names, and the attestation/receive role (ADR-0036) stays on this key directly.

What it is **not** (corrected once real implementations are in, see
Implementation): the homeserver keeps its *own* federation signing key, and
mautrix-go's crypto maintains its *own* Curve25519/Ed25519 **E2EE device keys**.
Those are operational keys the libraries own; the device key *authorizes* and
*cross-signs* them rather than *being* them. So it is one root identity binding
four roles, not one literal keypair doing four jobs — and ADR-0035's "hide the
key from the agent's own tools" denylist protects that root, for free.

### D4 — `Principal`/`Origin` go load-bearing; `Author` is `sender`

The reserved fields are filled by the substrate, not threaded through call sites
later (the migration ADR-0019 was paying to avoid): `Session.Principal` = the
authenticated Matrix `sender`; `Origin` = its homeserver/door. ADR-0038's
`Author` *is* the room event `sender` — and ADR-0038's load-bearing rule
("stamped server-side, never trusted from the client frame") is precisely how
Matrix stamps `sender`. The bespoke `sanitizeGuestName`/`filterShareFrame`
apparatus dissolves into homeserver-stamped identity.

### D5 — intents are inbound room events; permission is power levels × `Grant`

A `PromptIntent` is an `m.room.message` (or `life.arbos.prompt`) from a member
who holds *drive* power; `SteerIntent`/`InterruptIntent`/`ApprovalResponse`/
`QuestionResponse` are custom events correlated by `RequestID`. The actor's
inbound channel is fed by the Matrix sync adapter. Permission is two layers,
never collapsed (ADR-0034's "trust decided at the node" survives intact):

1. **Room (coarse):** `m.room.power_levels` decides who may *send* a drive
   event at all — the homeserver enforces it before the agent sees anything.
2. **Node (fine):** even for a permitted sender, the agent maps
   `sender → Grant` (ADR-0013): owner → full toolset; trusted guest →
   restricted tools + capped `Budget`; stranger → refuse or chat-only. The room
   decides who may *ask*; the agent decides what it will *do*.

### D6 — outbound: room events, with streaming kept local (the honest leak)

The engine writes `Room`-audience events (final assistant messages, shared tool
results, `life.arbos.task`/`kernel_event` for delegation). **Token-level
streaming is not a federation primitive** — Matrix events are discrete. So
`MessageDelta`/`ReasoningDelta` remain a *local client* concern (an agent
streams to its own attached frontends over its local homeserver), and the room
receives the coalesced message or in-place edits — the exact send-early,
grow-by-edit pattern the Telegram door already runs (`editMessageText`). This
seam is named, not papered over.

### D7 — federation rides the forest relay; the commons is a second hat

The forest relay (ADR-0034 Layer 3) already proxies inbound HTTP/WS to a NAT'd,
outbound-only node — exactly the inbound path a homeserver needs to federate. So
`server_name = kitchen.arbos.life`, `.well-known/matrix/server` delegates to the
tunnel, the `*.arbos.life` wildcard cert terminates federation TLS, and A↔B
federation routes through each side's tunnel. The relay stays *transport, not a
trust boundary* — every event and request is signed by the node's own key, so a
compromised relay denies or (pre-E2EE) observes, never impersonates (ADR-0034,
unchanged). Two hats reconcile local-first with big rooms: **your node's
homeserver** holds private/working rooms (your disk); **your human identity's
account on the arbos.life commons** holds thousand-member rooms (a beefy server
you moderate), where the frontend connects as a *client*, not by federating your
node into the crowd.

## Subtraction (do this before scaffolding)

What a from-scratch Matrix-native arbos never builds, and what we therefore
retire as the adapter lands:

- the **control-seam wire codec** for remote agents → federated events;
- most **per-door machinery** in `internal/messenger` / `internal/share`
  (share links, guest-name stamping, origin-based echo suppression) → room
  membership, power levels, Matrix transaction-id dedup;
- the **forest directory + heartbeat-as-presence** custom protocol → Spaces /
  room directory + Matrix presence (the identity challenge survives only to
  mint a Matrix token);
- a separate **attestation/identity transport** → signed Matrix events.

## What stays untouched (the proof the seam is right)

The cognition core: the turn loop/engine, `LLMProvider`/`ToolRuntime`/`Clock`,
`Agent`/`delegate`/`Grant`/`Budget`/`Environment`, `Project`, compaction, tools,
skills, memory. Matrix has nothing to say about how an agent thinks. **The
kernel must never import a Matrix type** — all translation lives in the adapter,
symmetric with the LLM provider adapters (the day `mautrix` appears in
`internal/core`, this design has failed).

## Implementation: mautrix-go + gomuks/hicli

Neither library is a homeserver — both are **client-side**, which is exactly the
layer arbos needs, and they correct this ADR's earlier "embedded homeserver per
node" framing:

- **mautrix-go** (`maunium.net/go/mautrix`) — the Go Matrix framework: the
  client-server `mautrix.Client`, the `event` package (where the custom
  `life.arbos.*` types register), the **appservice** API (owns the
  `@*:arbos.life` namespace, mints per-node tokens bound to `AgentID` via the
  ADR-0033 challenge), and **crypto** (goolm/`OlmMachine`, cross-signing) — so
  E2EE is a config flag, not a build.
- **gomuks/hicli** (`go.mau.fi/gomuks/pkg/hicli`) — gomuks's *headless
  local-first client*: it syncs a homeserver into a **local SQLite room cache**
  (`database.Event`), manages crypto and push rules, and exposes a JSON-command
  API (`send_message`, `send_event`, `set_state`, `paginate`, `get_room_state`,
  `get_event` — DB-first with homeserver fallback — `create_room`, `join_room`,
  `get_space_hierarchy`) plus a backend→client event stream (`sync_complete`,
  `client_state`) over a websocket **or an in-process channel**.

The mapping:

- **`SessionStore` (networked impl) = hicli's local DB.** `Room`-audience reads
  come from hicli (`get_room_state`/`paginate`/`get_event`); `AppendEvent` of a
  `Room` event is `send_message`/`send_event`. `Local`-audience events
  (reasoning, usage, context, compaction summaries) **never reach hicli** — they
  live in arbos's own small per-room table. `Project` folds the two. The D1
  split is precisely what keeps the private trajectory out of the Matrix store.
- **The door = hicli's sync loop, embedded in-process.** Its `sync_complete`
  events feed the actor's inbound channel; the in-process channel mode (not the
  websocket) means hicli runs *inside* the arbos process — the same shape as
  arbos's existing `gateway` + `web/` backend/frontend split, so the arbos UX is
  kept and hicli is the data engine beneath it.
- **Streaming stays local (D6).** Token deltas render to the node's own attached
  frontend off the engine's event stream; the room receives the coalesced
  message via `send_message` (or an edit), never 50 events.

Two hard rules carry over: **no mautrix/hicli type ever enters `internal/core`**
(the adapter translates `database.Event ⇄ core.Event`, `core.Intent → send_*`,
symmetric with the LLM provider adapters), and beware the **naming collision** —
mautrix's appservice `Intent` ("act as this user") is unrelated to `core.Intent`
("ask a session to do something"); keep them in separate packages.

## Seamlessness (no flag day)

- **Offline/solo never hard-depends on Matrix.** `SessionStore` stays a port
  with two reference impls; `arbos .` offline resolves the SQLite store, exactly
  as ADR-0033's resolution order keeps the `fake` tier. The Matrix impl needs a
  reachable homeserver, so it is the *networked* path, not a prerequisite — and
  because hicli keeps a full local replica, a networked session still reads its
  own history offline (it just can't send until the homeserver returns).
- **Old logs decode unchanged.** `Audience` is additive with `AudienceLocal`
  zero value (ADR-0010): no version bump, no upcaster, today's behavior is "all
  Local," i.e. nothing federates until an event is explicitly raised to `Room`.
- **Migration is a projection, not a rewrite.** A SQLite session becomes a room
  by replaying its `Room`-audience events into a freshly created room; the
  `Local` overlay stays local. Because the room is just another event log, the
  import is a fold, not a schema migration.

## Phasing (sequenced for option value — scaffold first)

- **P0 (scaffold; every later phase needs it):** the `Audience` field + the
  `ProjectEvent`-vs-`Audience` split; the one-key identity unification (D3);
  confirm `SessionID`-as-room-id needs no core change. Pure data-structure work,
  no homeserver yet.
- **P1:** the hicli/mautrix-go `SessionStore` adapter against a **central
  arbos.life homeserver** (Dendrite or Conduwuit), *not* a per-node homeserver —
  each node holds a full local hicli SQLite replica (local-first reads, offline
  history) while canonical storage + federation live on the one server. Every
  session is a room; the arbos frontend renders the room timeline. This is much
  lighter than embedding a homeserver and proves the substrate swap fast. The
  homeserver is reached client-server only, so its *location* is one base URL —
  moving to a **per-node Dendrite** later (true canonical local-first) is a
  config change, not an agent rewrite (option value, D2).
- **P2:** humans + multiple agents in one local room via membership + power
  levels + `sender→Grant`. *Subtract* the bespoke share machinery.
- **P3:** federation over the forest relay; cross-node rooms; remote delegation
  as federated `task`/`kernel_event`. *Subtract* the control-seam codec.
- **P4:** the arbos.life commons homeserver (big rooms, two hats); directory as
  Spaces. *Subtract* the forest directory protocol.
- **P5:** wallet/attestation as signed events; agent-to-agent payments
  (ADR-0036 P4).

## Amendments to existing ADRs

- **ADR-0033:** the account's node directory becomes room/Space membership; the
  device key gains the homeserver-signing-key role (D3). Resolution order and
  the offline `fake` tier are unchanged.
- **ADR-0034:** the relay additionally proxies Matrix federation (S2S); auth
  stays node-side; the "anonymous vs linked name" tiers map to local-room vs
  commons-account hats. The control seam's remote-agent role is superseded by
  federation.
- **ADR-0035:** `AgentID` additionally derives the Matrix localpart and
  room/space names; "stable-while-live URL" extends to "stable-while-live
  homeserver." The tool denylist now also guards the federation key.
- **ADR-0036:** attestations and payments are signed Matrix events on the same
  one key; P4 (agent-to-agent payments) gets its transport for free.
- **ADR-0013/0038:** delegation rides federated events instead of the bespoke
  seam; `Author`/`chat_note`/projection-exclusion are subsumed by `sender` and
  the `Audience`×projection 2×2.

## Alternatives rejected

- **Bolt Matrix on as the Nth messenger door (Telegram-shaped).** Works for
  human↔agent chat, but leaves the SQLite log, the share mechanism, the forest
  protocol, and the control seam all standing in parallel — the opposite of the
  net-subtraction this design exists for. The point is that these *are* the same
  thing.
- **Room as pure transport, kernel session remains canonical.** Keeps two event
  logs forever and forces a sync/dedup layer between them; loses the "compaction
  is a local overlay on immutable shared history" simplification of D1.
- **Federate everything (no `Audience` axis).** Leaks the model's private
  trajectory (reasoning, usage, injected memory) onto the homeserver and to
  peers. The whole point of D1 is that federation visibility is a first-class
  per-event property.
- **Per-node homeserver for big public rooms (no commons hat).** A 1000-member
  federated room melts a tiny per-node homeserver (state-resolution + fan-out
  cost). Two hats — node for private, commons for crowds — is the only topology
  that keeps local-first *and* scale.
- **A new mesh protocol (finish the forest's own wire).** That is what we are
  *deleting*; Matrix is a decade of solved federation, attribution, and
  moderation we would otherwise reimplement badly.
```
