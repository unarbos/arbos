# ADR-0041 — The room is the session: every arbos is a Matrix homeserver

- Status: Proposed (sketch)
- Date: 2026-06-26
- License impact: **adopts AGPL-3.0 for arbos** (was MIT) — required to bake in
  Dendrite (AGPL) and the mautrix/gomuks stack; see License.
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

So this ADR does **not** graft Matrix onto arbos. It finishes the job the
hexagonal boundary was already drawn for, and makes one decisive choice that the
earlier drafts circled: **the Matrix homeserver is not a service arbos talks to —
it is compiled into every arbos.** Each node *is* a homeserver (Dendrite,
in-process); arbos.life is just a bigger node of the same binary. That single
choice is what turns this from "add a feature" into "delete five subsystems," so
the bulk of this ADR is a subtraction ledger.

The expensive thing to get wrong is the **data shape**, so per foundational
thinking the Decision leads with types, not plumbing.

## Decision

> **The room is the session, and every arbos is the homeserver that owns its
> rooms.** The kernel projects a room into a turn and writes its room-visible
> output back as events. Identity, attribution, multi-party, permissions,
> directory, federation, delegation transport, and attestation are one
> substrate — an embedded Matrix homeserver — and the cognition core does not
> change. arbos stops being a *client* of Matrix and becomes a *peer*; a peer
> has no "local vs networked" mode, so the system's dualities collapse.

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
  agents and even a remote homeserver never see them. The model's inner
  monologue is not federation traffic.

The model a member sees, then, is: **shared room events (`Room`) interleaved
with that member's own local overlay (`Local`)**, folded by the existing
`Project` in timeline order. `chat_note`'s projection-exclusion stops being a
special case and becomes one cell of a 2×2.

**Room footprint and what "canonical" means (taking `Audience` seriously at turn
and lifecycle scale).** Two clarifications fall out, and together they give one
principle: *the room is a lean, shared, bounded **window**; the node's local store
is the full, private, durable **record** — minimize what federates, keep fidelity
local.*

- *Granularity.* A turn's **room footprint is exactly its `Room`-audience events**
  — by default just the prose (user + assistant `m.room.message`). Tool activity
  is `Local`: the arbos client renders rich cards from its overlay; a generic
  client (Element) sees clean prose. The "N unsupported `tool_result` events"
  worry only arises in a *shared working room* where results are raised to `Room`,
  and there each carries an **`m.notice` fallback `body`** ("🔧 ran `grep`") so
  foreign clients get a one-liner, not an unknown event. The assistant event's
  `life.arbos` extension lists `tool_calls` as correlation metadata. No
  edit-coalescing needed.
- *Longevity / "canonical".* The room is the canonical **shared** log — immutable
  while live, but **retention-boundable** (`m.room.retention`) as a
  federation/storage policy, because an unbounded DAG replicated to every peer is
  a real cost. The canonical **trajectory** is *not* the federated room; it is the
  **owner's durable local capture** (room events its node archived + the `Local`
  overlay + the D14 backup), which retention never touches. So D1's earlier "the
  room is the pristine record" sharpens to **the room is the shared window; the
  local replica is the trajectory of record.** A new collaborator gets the
  retained window (correct — not months of backlog); the owner keeps everything.

### D2 — one store: the in-process homeserver (the duality that disappears)

`core.SessionID` is already an opaque distinct type, not a bare string, so it
holds `!roomid:server` with **zero kernel change** — option value banked years
early. (D11 generalizes this: a `SessionID` is a *projection scope* — a room, or
a room + thread root for a nested scope.) Because the homeserver is *baked in*,
there is now exactly **one** `SessionStore` in production, not a solo-vs-networked
pair:

- **Room-audience events** live in the embedded Dendrite (its own SQLite, in
  `.arbos/`). A solo `arbos .` is a homeserver with one local user and no
  federation peers; it is not a different code path, just a homeserver with an
  empty membership beyond you. There is no "offline store vs online store" fork
  to maintain, because your homeserver is always *right there* on loopback.
- **Local-audience events** (reasoning, usage, context, compaction summaries)
  stay in arbos's own small per-room overlay table — the same `.arbos/` brain
  that already holds mind/atoms/plans/jobs (ADR-0035), which is *unchanged*:
  rooms absorb the conversation log, not the agent's private brain.

`Events` folds the two (room timeline + local overlay); `AppendEvent` routes by
`Audience`. `Seq` shifts from store-assigned monotonic to the room's timeline
ordering; `Project` already tolerates reordering and unknown payloads
(ADR-0010), so this is contained. `SessionStore` stays a **port** (option value:
swap the embedded server, keep `fake.Store` for tests) but production has one
reference impl, and the dual-store branching is *deleted*.

**Concurrency corollary (foundational thinking).** The actor model holds
*tighter*, not looser: each agent's session actor is the **sole writer of its
own events** (its `Local` overlay and the `Room` events it authors). The room is
multi-writer *across* agents/servers, and that merge is exactly what Matrix's
DAG + state resolution exist to do — arbos does not invent a second
reconciliation mechanism. "No shared mutable session state" (05-concurrency)
survives because no two actors write the same event.

### D3 — two identity roots: the machine (device key) and the person (recovery key)

There is no single root key — there are **two**, by kind, and conflating them was
the old confusion (see D9). Both are clean because, owning the homeserver, we
control its keys; each derives its subordinate keys with a context tag (HKDF over
the seed) — proper domain separation, not reuse.

**The machine root** is the device ed25519 key (ADR-0033), deriving everything
tied to *this box*:

- **homeserver federation signing key** = `derive(device,"matrix/federation")`,
  so the server *is* the machine (D8);
- each **agent user's master cross-signing key** =
  `derive(device,"matrix/user/<agentID>")`, so a wiped node regenerates the same
  agent identities from the same device key — no separate agent-key backup;
- **AgentID** = `H(devicePub‖dirPub)` (ADR-0035) → the agent's Matrix localpart;
- **attestation / receive address** (ADR-0036) for the agent.

It stays in `.arbos/`, never travels, and ADR-0035's "hide the key from the
agent's own tools" denylist guards it.

**The person root** is the human's Matrix cross-signing master key (`@const`),
anchored in a **recovery key the human holds** — not in any machine — with its
private half in Secret Storage on an always-on home server (D9). This is the one
identity arbos never modeled before (pre-Matrix the human was implicit in "the
operator"); being device-independent, the human survives any node's loss.

Two roots, cleanly separated: machine keys are regenerable and stay on the box;
the person's recovery key is the one backup that matters. Matrix E2EE device keys
(per client) are cross-signed under whichever root owns them.

### D4 — `sender` is the one identity field (`Principal` + `Origin` collapse into it)

ADR-0038's `Author`, the reserved `Session.Principal`, and the reserved
`Session.Origin` are all the same thing the substrate already stamps: the Matrix
**`sender`** (`@user:server`). It carries the principal (`@user`) *and* the origin
homeserver (`:server`), server-stamped and unforgeable — exactly ADR-0038's
load-bearing rule ("stamped server-side, never trusted from the client frame").
So rather than *fill* two reserved fields, we **collapse them into one** (D15a):
`Principal` becomes `sender`; `Origin` is **removed** (the homeserver is already
in `sender`; the old "door" sense — `telegram:chat/123` — is now a bridge user /
room membership). The bespoke `sanitizeGuestName`/`filterShareFrame` apparatus
dissolves into your own homeserver's stamping.

### D5 — intents are inbound room events; permission is power levels × `Grant`

A `PromptIntent` is an `m.room.message` (or `life.arbos.prompt`) from a member
who holds *drive* power; `SteerIntent`/`InterruptIntent`/`ApprovalResponse`/
`QuestionResponse` are custom events correlated by `RequestID`. The actor's
inbound channel is fed by the homeserver's sync stream. Permission is two
layers, never collapsed (ADR-0034's "trust decided at the node" survives
intact, and is now literal — the node *is* the server):

1. **Room (coarse):** `m.room.power_levels` decides who may *send* a drive event
   at all — your homeserver enforces it before the agent sees anything.
2. **Node (fine):** even for a permitted sender, the agent maps `sender → Grant`
   (ADR-0013): owner → full toolset; trusted guest → restricted tools + capped
   `Budget`; stranger → refuse or chat-only. The room decides who may *ask*; the
   agent decides what it will *do*.

D12 makes this concrete — the tier table, where the policy lives, the
`Grant`-for-senders symmetry, and how it composes with approval routing.

### D6 — outbound: room events, with streaming kept local (the honest leak)

The engine writes `Room`-audience events (final assistant messages, shared tool
results, `life.arbos.task`/`kernel_event` for delegation). **Token-level
streaming is not a federation primitive** — Matrix events are discrete. So
`MessageDelta`/`ReasoningDelta` remain a *local client* concern (an agent streams
to its own attached frontends straight off the engine's event stream), and the
room receives the coalesced message or in-place edits — the send-early,
grow-by-edit pattern the Telegram door already runs. This seam is named, not
papered over.

### D7 — every arbos is a homeserver; arbos.life is a bigger one (no two hats)

The forest relay (ADR-0034 Layer 3) already proxies inbound HTTP/WS to a NAT'd,
outbound-only node — exactly the inbound path a homeserver needs to federate. So
`server_name = kitchen.arbos.life`, `.well-known/matrix/server` delegates to the
tunnel, the `*.arbos.life` wildcard cert terminates federation TLS, and A↔B
federation routes through each side's tunnel. The relay stays **pure transport**:
every event and request is signed by the node's own derived key, so a
compromised relay denies or (pre-E2EE) observes, never impersonates.

The earlier "two hats" for *rooms* (own homeserver vs an account on a central
commons) **collapses**: there is one model — you are always a homeserver, the
unit being the machine (D8) — and `arbos.life` runs the *same binary* on a big
box. It is not a different server type; it is a node with more CPU that happens
to host large rooms (its server carries them, others federate in). "Same server
everywhere" is literal: same code, uneven hardware. (Homing the *human* identity
on an always-on server, D9, is a separate thing — identity homing, not a room
hat; rooms still live wherever created.) The cost is honest and bounded:
Dendrite's large-room ceiling caps the big-room experience until it improves (see
Risks), and if that ever proves fatal the swap is *uniform* — but in-process
embedding is what locks us to Dendrite, so that swap also surrenders the
single-binary property.

### D8 — the homeserver is the *machine*; the per-dir name is a web entry point, not a server

The unit that runs a homeserver is the **machine**, not the dir — one embedded
Dendrite per box, not N (per-dir is untenable: N homeservers, N DBs, and
same-machine cross-dir collaboration would have to *federate with itself* over
the relay; see Risks). This resolves the knot that the forest's per-(dir,machine)
name was doing three jobs at once — public web URL, agent identity, and (now)
federation `server_name` — by splitting them along the axis ADR-0035 already
prizes (router ⟂ relay):

- **`server_name` = `<machine>.arbos.life`**, derived from the device key alone
  (`agentDigest(devicePub, nil)`, already supported in `forest/identity.go`).
  One homeserver, one cert (the existing `*.arbos.life` wildcard), one
  `.well-known`, one key endpoint, one federation tunnel per machine.
- **The per-dir subdomain stays — as a *web client entry point*.** `arbos .` in
  project X still leases its deterministic `fuzzy-otter-3f2.arbos.life`, and that
  subdomain tunnels to the node to serve the **web UI scoped to that dir's
  agent-user and rooms**. The ADR-0035 headline UX (same project → same URL, no
  login) survives verbatim, now decoupled from federation: **web-URL ⟂
  `server_name`.**
- **Dir-agents are users** `@<agentID>:<machine>.arbos.life`. Two dirs on one box
  collaborate in a **local room** (no federation); their private brains
  (mind/atoms/plans in `.arbos/`) stay per-dir, so mind-separation (ADR-0035) is
  untouched — only the Matrix transport/storage layer is shared.
- **Per-dir homeserver is the opt-in isolation tier.** A project that needs a
  separate federation identity / blast radius runs its own embedded Dendrite;
  because the homeserver sits behind the `SessionStore` port, that is a config
  choice, not a fork. Default one-per-machine; isolation is the addition — the
  standing "freedom is baseline, safety is the addition" posture (ADR-0013).

The forest lease shifts from per-dir to **two leases per active machine** — the
machine-HS lease (the `server_name`) plus each open project's web-entry lease —
both already expressible via `agentDigest` with and without the agent key, so
`head.go` barely changes.

### D9 — who is the user: person, machine, and agent are three identities

"Device" was overloaded, and that was the whole confusion. arbos's "device"
(ADR-0033, a machine's root secret) and Matrix's "device" (a user's E2EE login
session) are different layers; once named apart, ADR-0033's "one account per
device" simply reads as "one homeserver per machine" (D8), and the human is free
to be its own identity. Three roots, three lifecycles:

| Identity | Rooted in | Homed on | Backup |
|---|---|---|---|
| **Person** `@const` | a recovery key the human holds | an **always-on** server (commons default; any always-on node self-hosts it) | the recovery key — the one that matters |
| **Machine** `<machine>.arbos.life` | the device key (D3) | itself | regenerable; stays in `.arbos/` |
| **Agent** `@<id>:<machine>…` | `derive(device,"matrix/user/<id>")` | its machine | regenerable from the device key |

The person is homed on an always-on server because identities and agents have
opposite lifecycles: an **agent** is tied to a running node (offline when the
node sleeps — fine, it isn't running anyway), but a **human must be reachable
from a phone at any hour.** This is *identity homing*, not a room hat (D7).

**Multi-device** is then standard Matrix: each human client (phone Element, a
trusted node's web UI) is a Matrix *device* cross-signed under `@const`'s master
key; **key backup** (server-side, recovery-key-unlocked) gives a new device its
history. Two arbos-specific points:

- **Auto-trust your own agents.** Sharing an E2EE room with `@kitchen` would
  normally demand manual device verification. Because linking (ADR-0033 P2)
  already proved "this human owns this machine," arbos auto-cross-signs the
  agent's device for `@const` — *your* agents are pre-verified, strangers' are
  not. This is the one genuinely new seam (not adopted from Matrix), and it rides
  the existing link proof.
- **Web-UI E2EE is trust-scoped.** A web client acting as `@const` makes the node
  hold `@const`'s E2EE keys — fine on your own machine, not on a hosted one. So
  the web UI does full E2EE only on the human's own trusted node; elsewhere the
  human's real E2EE device is a proper client (Element) and the web view is
  thinner. This is the honest boundary of "the web UI is just a Matrix client."

### D10 — the `life.arbos.*` wire contract: three channels, not one stream

Refines D5 (inbound) and D6 (outbound). The kernel has three vocabularies with
three lifecycles — `Intent` (in), `KernelEvent` (out), `EventPayload`
(persisted) — and Matrix has three channels that match them one-for-one, so the
schema is "route each vocabulary to its channel," not "map every type to a
timeline event."

| Kernel vocabulary | Lifecycle | Matrix channel |
|---|---|---|
| **content** (prompt, steer, chat_note, assistant, tool_result) | durable, shared, attributed | room timeline |
| **control** (interrupt, compact, set_model, set_web*, resume) | directed, ephemeral, one agent | to-device (`/sendToDevice`) |
| **presentation** (deltas, progress, queued) | ephemeral, local render | local stream (never federated — D6) |

Three sub-decisions, settled:

1. **Intents: content is a room event, control is to-device.** A prompt/steer is
   an `m.room.message` the agent reacts to *as a member* (uniform — humans and
   agents both just send room messages; the agent acts on those it is addressed
   in, D5). Control commands are **to-device** messages: directed at one agent,
   ephemeral, no timeline clutter, and they **federate**, so the same primitive
   drives a *remote* agent. (Rejected: everything-as-room-event leaves control
   noise in an immutable DAG; intents-stay-private breaks attribution and remote
   control.)
2. **One dual-readable event, not two.** A Matrix event's `content` is an open
   object, so prose is a **single `m.room.message`** carrying both
   `body`/`formatted_body` (Element renders) *and* a `life.arbos` extension key
   (reasoning, tool_calls, provider_meta, citations, usage). No parallel shadow
   event, no dedup. Things with no prose form (tool_result, task, kernel_event)
   are custom `life.arbos.*` types (Element hides them; an optional `m.notice`
   can summarize tool activity for human clients).
3. **Flat timeline + metadata; threads for delegation depth, not turns.** The
   conversation is linear; `life.arbos.turn_id` groups a turn and
   `m.relates_to: m.reference` links a `tool_result` to its assistant call
   (mirroring `ToolResult.CallID`). **Threads are reserved for delegated
   sub-agents** (Envelope.Depth > 0, ADR-0013), giving `Depth` a native
   rendering. Streaming coalesce (D6): default sends the final assistant message
   once; an optional live mode grows it via `m.replace` edits (a tunable — each
   edit is its own event).

The contract:

```
ROOM TIMELINE (durable, Audience=Room, attributed by sender)
  m.room.message          user/assistant prose; content += life.arbos{turn_id, role,
                          reasoning?, tool_calls?, provider_meta?, citations?}
  life.arbos.tool_result  {call_id, content, is_error}; relates_to m.reference → assistant
                          Audience: LOCAL by default (not in the room — arbos renders
                          from the overlay; Element sees only prose). When raised to
                          Room (shared work), carries an m.notice `body` fallback
                          ("🔧 ran grep") so foreign clients show a one-liner — D1.
  life.arbos.chat_note    human-to-human; excluded from model projection (ADR-0038)
  life.arbos.question     {request_id, questions|approval, reason}
  life.arbos.answer       {request_id, answers}; relates_to m.reference → the question
  life.arbos.task         delegation Task          (in a thread / child room — depth)
  life.arbos.kernel_event relayed child events     (depth)

TO-DEVICE (directed, ephemeral, federates to remote agents)
  life.arbos.control      {op: interrupt|compact|set_model|set_web_*|resume, args}

LOCAL STREAM (never federated — D6)
  message_delta, reasoning_delta, tool_progress, tool_details,
  citations/images(partial), queued
```

The one genuinely novel seam: **to-device for control** means the adapter wires a
second inbound source (to-device) into the actor's intent channel alongside
room-sync — worth prototyping early, since it touches the actor loop.

### D11 — room ↔ session granularity: map each tree edge to the matching primitive

arbos sessions form a **tree with two edge kinds**, and the mapping follows each
edge's semantics rather than forcing one rule:

- **divergent** (fork, ADR-0027): an independent, rebinding line → its **own room**;
- **anchored/nested** (branch ADR-0037, delegation ADR-0013, owned spawn) → a
  **thread** in the parent room (rooted at the relevant event).

| arbos relationship | Matrix primitive |
|---|---|
| root chat | room |
| fork (ADR-0027) | new room (Space child) + `life.arbos.forked_from`, seeded by re-emission |
| branch (ADR-0037) | thread rooted at the highlighted event; projection scoped to the seed |
| delegation (ADR-0013) | thread by default (renders `Depth`); child room when it needs its own membership (a remote agent) |
| owned spawn (`Owner`=chat, scheduler) | thread in the owner's room, tagged `life.arbos.spawned_by` |
| ownerless spawn (`arbos -once`) | its own room (often `Audience=Local`, maybe never federated) |
| the session tree | a per-agent **Space** (children = root + fork rooms) |
| session end (`SessionEnded`) | **leave/archive** (out of the active Space), **never delete** |

This generalizes D2's key: **a `SessionID` is a projection scope** — a room
(`!room:server`) for a top-level scope, or a room + thread root
(`!room:server/$event`) for a nested one. A thread-scope's projection is the
thread's events + its seed segment (exactly `sessiontree.Branch`'s seed), so a
branch lives *in* the parent room, visibly anchored to the highlight, while the
model still sees only the fragment — projection, not the room boundary, controls
context. One actor per scope, each the sole writer of its own events into the
shared DAG (D2 corollary).

Two wrinkles, named: **fork is copy-by-re-emission** — Matrix events are
sender/server-signed and there is no "share history then diverge" primitive
(tombstone *replaces*), so a fork's prefix is re-emitted as `life.arbos.imported`
events authored by the forker (acceptable: a fork is a divergent copy). And
**never delete** — `SessionEnded` leaves/archives; the room and its history
persist for replay, consistent with the append-only log.

### Consequence — subagents and trajectory (D1 × D10 × D11)

Two cross-cutting capabilities fall out of the Audience split (D1), the wire
(D10), and the granularity map (D11):

**Subagents** are D11 + D10 + D7: a child is a thread (or child room) running a
`life.arbos.task`, relaying `life.arbos.kernel_event`s up by `Depth`, controlled
over to-device; local and remote children are the same "invite a user to a room"
(D7). A subprocess `TransportAgent` (Codex/Claude/Cursor) is captured at the
relay boundary — its events become room + overlay records like any child's.

**Trajectory extraction** is almost entirely D1, with two consequences:

- **The trajectory is room (raw) + that agent's `Local` overlay, and it is
  per-agent.** Reasoning, usage, `ConfigPayload`, injected context, and
  `provider_meta` (ADR-0003) are `Local`, so the **room alone is not a complete
  trajectory** — the training-grade detail lives in the overlay, and two agents
  in one room have *different* trajectories (same room, different private
  detail). Extraction must be scoped to *whose*.
- **Compaction is `Local`, so the *trajectory* is pristine and uncompacted** —
  the owner's local capture (archived room events + overlay) never loses events to
  context management; an agent's summary is a private overlay on top. (The
  *federated* room may be retention-bounded, D1 refinement — fidelity lives in the
  local record, not the shared window.)

**#12 — `Local`-overlay durability (resolved by D12).** The private trajectory
detail does **not** federate, so it lives on one node: the shared conversation
survives a node wipe (replicated to peers), but reasoning/usage/config/
`provider_meta` do **not** — losing training detail and the ability to
round-trip thinking signatures on resume (ADR-0003). **D12 folds this into one
node-private-state backup** (overlay + E2EE keys + `provider_meta`, encrypted to
the always-on home server, unlocked by D9's recovery key) — the trajectory analog
of D9's key backup, now unified with E2EE key backup.

### D12 — the agent membrane: inbound `Grant`, outbound exposure gate, channel E2EE

The security core (subsuming D5) is **one boundary seen three ways**: every agent
is a *membrane* between its private interior (Local overlay, secrets, tools,
reasoning) and the room. Each control reuses an existing choke point, and one
symmetry runs through all three — **coarse/shared rules live in the room
(Matrix-native, server-enforced); fine/private rules live at the node** (ADR-0034).

**Inbound authority — `sender → Grant`.** A member's authority over an agent *is*
a `Grant` (ADR-0013) — the same token delegation hands a child, now conferred by
membership instead (delegation hands a Grant *down*; membership confers one on a
*member*). Two layers:

- **Coarse (in-room): power levels.** `m.room.power_levels` gate who may send
  which event type, enforced server-side before the agent sees anything.
- **Fine (at-node): a `.arbos/` policy** mapping `(sender, power level) → Grant`:

| PL | who | Grant |
|---|---|---|
| 100 owner | the linked human `@const` | full toolset/budget, `auto` approval |
| 50 driver | trusted collaborator | curated toolset, capped `Budget`, `ask-owner` |
| 0 guest | any member | chat-only (no tools); converse, never act for them |
| non-member | — | nothing (membership gate) |

Approval composes: a driver's risky call → `ask-owner` → a `life.arbos.question`
(D10) whose **answer event requires PL 100**, so only the owner unblocks it (no
escalation by the asker); an offline owner ⇒ it stays suspended (suspend-await,
ADR-0018).

**Outbound exposure — `event → Audience`, guarded on the raise.** D1 defaults
tool results and private detail to `Local`, so nothing federates unless raised.
The Local→Room **raise is the one exposure point**, and it gets a redaction /
classification seam — known `SecretRef`s, broker-managed paths,
`.env`/`.pem`/key/high-entropy patterns → **block, redact, or owner-approve**
(same suspend-await). This is ADR-0016's "policy at the boundary," outbound: the
room is a destination secrets are never bound to. **E2EE does not cover this** —
encryption hides content from *servers*, not *members*; #6 (content) ⟂ #7
(channel), and you need both.

**Channel confidentiality — E2EE exactly when the boundary is federated.**
Local-only rooms (all members on your own homeserver) stay plaintext — the data
is already on your disk. Federated rooms get **E2EE on by default**: relay and
remote servers carry ciphertext only, closing ADR-0034's observe gap. The agent's
E2EE device key is **cross-signed by the machine root (D3)** — auto-verified, no
human in the loop — and human↔own-agent uses D9 auto-trust.

**One node-private-state backup.** E2EE keys + the `Local` trajectory overlay +
`provider_meta` are node-local material that doesn't federate; **D14 folds all of
it into the single SSSS-backed store** (one recovery key), covering
resume-after-wipe, E2EE history on a new device, and trajectory durability at
once.

In one line: **power levels say who may knock; a `Grant` says what they may make
the agent do; `Audience` + a raise-time redaction seam say what may leave; E2EE
(auto, when federated) says no foreign server may read the wall — escalation
always routes to PL 100, and the node's private material has one encrypted
backup.**

### D13 — multi-agent rooms: addressed-only activation, dual budget, hop-counter loops

"Agents working together at length" is **lateral delegation** — a room bounded
the way a delegation subtree is (ADR-0013), turned sideways — so it reuses
`Grant`/`Budget` rather than inventing a scheduler.

**Activation — addressed-only.** An agent takes a turn only when *addressed*: an
`@`-mention (`m.mentions`), a `life.arbos.task` targeting it, its own human
speaking in its room, or a reply in a thread it is in. It never reacts to an
unaddressed message, and never to its own (txn-id/`sender`, D10). Agents
collaborate by explicitly addressing each other, so every exchange is intentional
— this kills the token bonfire at the root. (Rejected: respond-to-all — bonfire +
instant loops; floor-control token — deterministic but rigid.)

**Budget — dual, both reusing D12.** An action must pass both caps:

- **per-member:** the participant's `Grant.Budget` (D12 / ADR-0013) — how much
  *that* agent may spend here;
- **collective (shared working rooms only):** **state for the cap, timeline ticks
  for the tally.** The **cap/policy** (token/turn/wall-clock/money ceiling + who
  may refuel) is a slow-changing room *state* event; the **running spend** is
  **append-only `life.arbos.spend` ticks** (`{agent, amount}` — Room-audience in a
  shared room, Local for a solo agent). The **fuel gauge = a fold of ticks against
  the cap** — append-only, so it is *conflict-free under federation* (concurrent
  decrements both just land; no last-writer-wins state-resolution lying about the
  total), the same append-and-project pattern as the event log. Empty ⇒ agents
  pause; refuel = the owner raises the cap (one tap). A normal 1:1 chat shows **no
  gauge** — budget is invisible until a shared working room needs it (the user's
  own spend is just their account balance).

**Loops — no self-trigger + a hop counter.** Beyond budget (the backstop),
distinguish **human-initiated** activity (unbounded — the human drives) from
**agent-initiated** activity (bounded): a message replying to *another agent*
increments a hop counter; **a human message resets it.** Past N agent-hops with
no human, agents **pause and check in** ("we've gone N rounds — here's where we
are; continue?"). The hop counter is the lateral analog of `Envelope.Depth`
(ADR-0013): a subtree is bounded by `Budget` + `Depth`; a room by per-member
`Budget` + hops.

### D14 — SSSS is the private-state substrate: one encrypted, recovery-key-unlocked, synced store

The secret vault, the node-private-state backup (#12), cross-signing keys, and
E2EE key backup (D9) are the **same need** — encrypted state, unlocked by the
person's recovery key (D9), synced across the account's nodes, unreadable by the
server. Matrix already has exactly this: **Secret Storage (SSSS) + key backup.**
So they are one substrate, not four:

- **Secret vault (ADR-0016):** secrets become encrypted **account data** in SSSS;
  the **broker resolves from SSSS at the HTTP boundary** (ADR-0016 unchanged — the
  agent sees only `SecretRef`s, only the trusted broker decrypts), synced to every
  node the person owns. The bespoke per-node AES-GCM vault file
  (`internal/secret/store.go`) becomes the **air-gapped/local fallback**, not the
  default.
- **Node-private-state backup (#12):** the `Local` overlay
  (reasoning/usage/config/`provider_meta`) + E2EE megolm keys ride the same
  SSSS-backed key backup — so resume-after-wipe, E2EE history on a new device, and
  trajectory-detail durability are one mechanism.
- **Cross-signing + device keys (D9):** already SSSS in Matrix; D14 just names
  them as the same store.

One recovery key (D9) unlocks all of it — a real subtraction the design had split
across D9/D12/#3/#12.

### D15 — the foundational collapse pass: remove core types by symmetry

With D1–D14 settled, a second look at the kernel's foundational primitives finds
several that the substrate makes *redundant* — places where two or more core types
now say the same thing. Per "data structures first / DRY the structure," these
collapse. Confidence is marked, because foundational thinking warns against
abstracting ahead of the wire: where the wire (D10) already unified a thing, the
core types should follow; where it didn't, verify call sites first.

**(a) Identity: `Principal` + `Origin` → one `sender` [high].** A Matrix
`@user:server` encodes *both* the principal (`@user`) and the origin homeserver
(`:server`), and the "door/surface" sense of `Origin` (e.g. `telegram:chat/123`)
is now a bridge user / room membership. So `Session.Origin` is **removed** and
`Session.Principal` *becomes* the Matrix `sender`. Two reserved fields (ADR-0019)
collapse to one populated one.

**(b) Lineage: `ParentID` + `Owner` + `SpawnedBy` → Matrix relations [medium —
verify call sites].** D11 already maps the session tree to Matrix: fork →
`life.arbos.forked_from`; branch/delegation/owned-spawn → a thread whose parent
*is* its room; the tree → a Space; spawn provenance → `life.arbos.spawned_by`.
Since a `SessionID` is now a scope (room or room+thread, D11), the parent is
*encoded in the id*. So the three bespoke lineage fields on `core.Session`
**collapse into Matrix relations** and become derivable. (Call sites to migrate:
`engine`, `plan/scheduler`, `agent`, `recall`, `piwire` read `Owner`/`SpawnedBy`
for routing/display — they re-read the thread's room / the tag instead.)

**(c) Control + capabilities: 4 `Set*Intent` → one `ControlIntent`; 3 toggles →
one set [medium].** `SetModelIntent`/`SetWebSearchIntent`/`SetWebFetchIntent`/
`SetImageGenIntent` are four near-identical intents (idle-apply, durable on the
session). D10 *already* unified them on the wire as one to-device
`life.arbos.control {op,args}` — so the core should follow: one
`ControlIntent{op, value}`. The mirroring `Session.{WebSearch,WebFetch,ImageGen}`
bools and the same three `ConfigPayload` fields collapse into one capability
set/map. (~6 parallel type surfaces → 2.)

**(d) Suspend-await: approval ⊂ question → one Request/Answer pair [high].**
`ApprovalRequest`/`ApprovalResponseIntent` and `QuestionRequest`/
`QuestionResponseIntent` are the same suspend-await machinery; an approval is a
degenerate yes/no question that gates a tool call. D10 *already* maps both to
`life.arbos.question`/`life.arbos.answer`. So the core collapses to **one
Request/Answer pair** (approval = a question carrying the gated `ToolCall`),
removing a type pair and an intent.

**(e) Tooling shrinks: both codecs serve only local state [high].** For
`Room`-audience content the *Matrix event is the persisted form*, so
`core/codec.go` (`EncodePayload`/`DecodePayload`) now serializes **only the Local
overlay**, and `core/interaction.go` serves **only the local stream + in-process
frontends**. Neither is deleted, but both shrink to their Local-only role — the
adapter owns Room serialization.

**(f) Confirmed dangling — delete:** `Queued` (kernel event) and prompt/message
`Origin` (echo-suppression) → room-event presence + txn-id/`sender`; sqlite
`boards`, `grants`, `outbox` tables; `internal/envfile`; the forest anonymous
`acct_…` account (machine identity *is* the device key now, D3 — the forest
`AccountID` concept is vestigial).

**(g) Stale contracts to refresh (not code, but dangling docs):** **ADR-0019**
(Principal/Origin *reserved*) is consumed and superseded by (a); **docs/02-data-model.md**
must be updated for `Audience`, `sender`, and the removed fields — otherwise the
canonical data-model doc drifts from the kernel.

Net of D15: the kernel sheds ~`Origin` + 3 lineage fields + 3 toggle fields + a
duplicate suspend-await pair + (effectively) 3 redundant `Set*Intent`s, and two
codecs shrink — all without touching the turn loop. Subtraction by symmetry, on
the data shapes, before any construction.

## Does this make it simpler? The symmetry dividend

Yes — decisively, and the mechanism is uniform: **a peer has no "local vs
networked" mode, so every duality in the system collapses to its single
federated case.** Complexity is removed *and* functionality is added by the same
move, because the functionality was Matrix's all along and we stop hiding it
behind bespoke halves.

### Dualities that collapse to one case

| Two things before | One thing now |
|---|---|
| solo store (SQLite) **vs** networked store (Matrix) | the in-process homeserver (solo = a homeserver with no peers) |
| local delegation (in-proc child) **vs** remote delegation (control-seam wire) | invite a user to a room (local = same server, remote = federated) — D-collapse of ADR-0013 |
| own homeserver **vs** account on a central commons (two hats) | everyone is a homeserver; arbos.life is the same binary, bigger |
| web door / TUI door / Telegram door / share link | Matrix clients of your homeserver |
| split agent-chat tab + a separate **People** side-chat surface (`PeopleSurface`/`PeopleChat`, its own scoped seam) | one room timeline — `chat_note`s inline as side-notes, roster = membership, presence native (P2) |
| forest = identity broker + directory + relay | relay = pure transport; identity = your server; directory = Spaces |
| client-of-central-server **vs** local-first cache (hicli) | your server is local, so there is nothing to cache — drop hicli |
| offline path **vs** online path | one path; your loopback homeserver is always present |
| `Queued` event + prompt `Origin` + cross-door echo-suppression | a prompt is a room event seen by all on send; self-suppress via txn-id + `sender` (D10) |
| three control transports (interrupt/steer/set_* over the seam) | one to-device `life.arbos.control` op that also federates (D10) |
| bespoke session-tree edges (fork / branch / delegation / spawn) | room / thread / Space by edge semantics (D11) |
| three security mechanisms (authz / DLP / encryption) | one membrane reusing `Grant` + `Audience` + identity roots (D12) |
| separate E2EE-key backup **vs** trajectory-detail durability | one node-private-state backup (D12, resolving #12) |
| a scheduler / floor-manager for multi-agent rooms | reuse `Grant.Budget` + a hop counter — lateral delegation (D13) |
| secret vault + node-private-state backup + cross-signing + key backup (four stores) | one Matrix SSSS store, one recovery key (D14) |
| bespoke outbox / share-links / panel-control transports | room events + invite/media + content extensions (Moves A); local FTS over your replica (C) |
| `Principal` + `Origin` (two reserved fields) | one Matrix `sender` `@user:server` (D15a) |
| `ParentID` + `Owner` + `SpawnedBy` (lineage fields) | Matrix relations / Space / thread parent, encoded in the scope id (D15b) |
| 4 `Set*Intent` + 3 toggle bools (×2) | one `ControlIntent{op,value}` + one capability set (D15c) |
| `ApprovalRequest`/`Response` + `QuestionRequest`/`Response` (two suspend-await pairs) | one Request/Answer pair — approval ⊂ question (D15d) |
| "the room is the full forever log + all tool guts" | the room is a lean, retention-boundable **shared window** (prose; tool detail `Local`); the local replica is the durable **trajectory** (D1 refinement) |

### Functionality that arrives for free (as a side effect of deletion)

- **Any Matrix client reaches your agent** — Element on your phone, Nheko,
  whatever — because it is a real homeserver, not a bespoke web UI.
- **The whole Matrix federation is reachable** — your agent can talk to any
  Matrix user on any server, not only other arbos nodes.
- **The mautrix bridge ecosystem replaces `internal/messenger`.** Instead of a
  hand-written Telegram door, you run a standard mautrix bridge and gain
  **WhatsApp, Telegram, Discord, Signal, Slack, …** at once. This is the
  sharpest "delete code, gain features" trade in the whole design: the bespoke
  per-platform door is removed and *every* platform arrives.
- **E2EE, presence, typing, threads, reactions, spaces, read receipts** — the
  full Matrix feature set, none of it hand-rolled.
- **Native offline** — your homeserver is local; you read and write your own
  rooms with no network, and federation backfills when peers return.

The net is a large subtraction with a large capability gain — the rare case
where "remove complexity" and "add functionality" are the same commit.

## What stays untouched (the proof the seam is right)

The cognition core: the turn loop/engine, `LLMProvider`/`ToolRuntime`/`Clock`,
`Agent`/`delegate`/`Grant`/`Budget`/`Environment`, `Project`, compaction, tools,
skills, memory, and the per-agent brain (mind/atoms/plans/jobs in `.arbos/`).
Matrix has nothing to say about how an agent thinks. **The kernel must never
import a Matrix type** — all translation lives in the adapter, symmetric with the
LLM provider adapters (the day a Matrix type appears in `internal/core`, this
design has failed). Baking the *server* into the *binary* does not bake it into
`internal/core`: the embedded homeserver runs on a goroutine bound to loopback,
and the adapter is its only caller.

## Implementation: Dendrite in-process + mautrix-go (hicli dropped)

- **The homeserver = Dendrite, embedded.** It is the only homeserver that is an
  importable Go library (Conduwuit/Synapse can only ever be subprocesses), and
  the maintainers steered it explicitly toward the embedded/P2P niche
  (monolith mode, SQLite, in-memory NATS) — exactly this use. arbos imports it,
  runs it on a goroutine bound to `127.0.0.1`, storage in `.arbos/`. "Same
  server everywhere, baked in" ⟹ Dendrite, specifically.
- **The client = mautrix-go** (`maunium.net/go/mautrix`), thin, talking to the
  loopback Dendrite: `mautrix.Client` for sync/send, the `event` package for the
  custom `life.arbos.*` types, `crypto` for E2EE.
- **hicli is dropped.** gomuks/hicli exists to keep a *local-first cache of a
  remote homeserver*. When the homeserver is local, there is nothing to cache —
  the server's own DB is the local store. Removing it also removes a dependency
  and a layer. (The AGPL decision means we *could* have kept it; we drop it for
  architecture, not licensing.)
- **Identity is self-served; the appservice mostly disappears.** Your own
  homeserver registers your own user (`@agent:kitchen.arbos.life`); there is no
  central appservice minting your identity. arbos.life's appservice survives only
  for the commons' own namespace, not as everyone's identity broker.
- **Streaming stays local (D6).** Token deltas render to attached frontends off
  the engine stream; the room gets the coalesced message via send/edit.

Hard rule and a trap: **no mautrix/Dendrite type enters `internal/core`** (the
adapter translates `event.Event ⇄ core.Event`, `core.Intent → send`); and beware
the **naming collision** — mautrix's appservice `Intent` ("act as a user") is
unrelated to `core.Intent` ("ask a session to do something").

## Topology: ephemeral vs always-on

Every capability has a **liveness requirement**, and that — not anything
Matrix-specific — decides where it runs. The system has exactly two tiers:

- **Ephemeral (the node):** the agent, its private rooms, its Local overlay.
  Offline when the machine sleeps; correct, since the agent isn't thinking when
  off.
- **Always-on (commons / home server):** anything that must be reachable when the
  node sleeps.

Three operational decisions are the same call — *place by liveness*:

**Ephemeral nodes ↔ federation (#9).** Vanilla Matrix federation + a **stable
`server_name`** (device-derived, D8) reconciles this: **the lease governs URL
*liveness*, not *delivery*.** A sleeping node 404s (lease lapsed), but sending
peers queue and retry for days against the stable name, which re-resolves through
the always-on relay/DNS the instant the node wakes and re-leases, then backfills.
Matrix presence renders the agent "offline" meanwhile. The relay stays a **dumb
pipe** (ADR-0034). For agents that must never miss a message, an **always-on room
mirror** (commons hosts the node's rooms) is a linked-tier reliability upsell —
not the default.

**Bridges (#10).** mautrix bridges are separate appservice processes, so "single
binary" holds for *node + embedded homeserver* and **bridges are optional
sidecars** (arbos's "genuinely external adapters are subprocesses by necessity"
rule, 01-architecture). They run on the **always-on tier**, not the ephemeral
node — a Telegram bridge that slept with your laptop would miss messages, exactly
like #9 — so they sit beside the always-on human identity (D9). Order: **Telegram
first** (a like-for-like replacement of `internal/messenger`), then WhatsApp /
Signal / Discord.

**arbos.life's residual role (#11).** Three always-on layers:

1. **forest head** — DNS/subdomain lease + TLS + dumb federation/web relay (pure
   transport, ADR-0034's ideal, now literal);
2. **the commons node** — the *same node binary* (embedded Dendrite + arbos),
   big, hosting public Spaces/rooms, the always-on human identities (D9),
   optional mirrors (#9), and bridges (#10);
3. **the account backend** (ADR-0033, shrunken) — **LLM routing/billing + the
   secret vault**, the irreducible central service, because money and secrets are
   the two things Matrix has no opinion about.

So **arbos.life = transport + a big commons node + a money/secrets backend** —
the first two the same binaries everyone runs (deployed always-on), the third the
only genuinely centralized piece.

## Resolved operational questions (the migration OPEN set)

The pre-implementation sweep ([docs/plans/03-matrix-migration](../plans/03-matrix-migration/surface-inventory.md))
surfaced ten operational questions the decisions above didn't fully settle. Each
folds into one of four Matrix-native moves: **(A)** content/sharing → room events
+ `life.arbos` extensions + invite/media; **(B)** private synced state → SSSS
(D14); **(C)** operate on the local replica, keep router ⟂ relay orthogonal;
**(D)** the in-process kernel `Intent`/`Event` boundary is the real seam, with
`Agent`/`Grant`/identity tokens reused.

| # | Question | Resolution | Move |
|---|---|---|---|
| 1 | Outbox / between-turn voice | Agent posts a room event (`m.room.message`/`m.notice`); Matrix push/unread delivers; broadcast notices → the agent↔owner DM room. DELETE `internal/outbox`. | A |
| 2 | Room full-text search | Local arbos FTS over the local Dendrite replica + overlay (repoint `events_fts`), scoped to joined rooms — not Dendrite's weak `/search`. | C |
| 3 | Central secret vault sync | Matrix SSSS (D14); broker resolves from it at the boundary (ADR-0016 unchanged); per-node file = air-gapped fallback. | B |
| 4 | show/ui frontend coupling | `show` → `life.arbos.surface` content extension (Room); `ui` → local-stream presentation (D6). Generic clients ignore the extension. | A |
| 5 | Control-seam local survival | DELETE the NDJSON wire; interactive TUI/web = Matrix clients of loopback; `-once`/ephemeral drives the engine **in-process** (no homeserver). The `core.Intent`/`KernelEvent` boundary stays the seam. | D |
| 6 | TransportAgent (Codex/Claude) | An `Agent`/`AgentTransport` wrapping a subprocess (MCP-style), relayed into a child thread as `life.arbos.task`/`kernel_event`; under parent identity (local) or a federated user (remote). | D |
| 7 | Scheduler vs room budget | A scheduled wake always draws the room `life.arbos.budget`; for hop-counting it is a **root** (human-intended), resetting the counter for its subtree; subtree bounded by per-wake `Grant.Budget`. | D |
| 8 | Machine ↔ billing link | Billing attaches to the **person** (`@const`), not the machine; the ADR-0033 P2 link flow binds device→person→account; per-agent spend = scoped tokens (ADR-0035/0036). | D |
| 9 | Default endpoint target | Two orthogonal axes (ADR-0035 router ⟂ relay): Matrix = **always loopback** (D8); LLM = the head by default (ADR-0033). The conflation dissolves. | C |
| 10 | sharedoc / trajectory share-links | Sharing = Matrix invite/media. DELETE the share-link entries; KEEP sharedoc render for authenticated file panels; trajectory export = owner CLI/API over room+overlay. | A |

## Design angles (all resolved) + record consolidation

A semantics-collapse read of the whole ADR surfaced four shapes the prior passes
glossed; all four are now resolved. Two folded **into D1** (the "room footprint
and what 'canonical' means" refinement): *turn granularity in the room* (a turn's
footprint = its `Room`-audience events — prose by default, tool detail `Local`,
`m.notice` fallback when shared) and *room growth vs the trajectory* (the
federated room is a retention-boundable shared **window**; the owner's local
capture is the durable **trajectory of record**) — the unifying principle *the
room is a lean shared window; the local store is the full durable record*.

The other two are now resolved too: **bootstrapping** is settled in
§"Bootstrapping: `arbos .` just works" (deterministic + idempotent + lazy
first-run creation, no login), and **`life.arbos.budget`** is settled in D13
(state for the cap, append-only `life.arbos.spend` ticks for the conflict-free
tally; gauge invisible until a shared working room needs it). So all four design
angles are closed; only a doc-hygiene item remains.

**Decision-record consolidation (on graduation sketch → Accepted).** The list has
accreted overlap to merge when finalized: **D5/D6** are previews fully covered by
**D10**; **D3** and **D9** both describe the identity roots; **D12**'s backup and
**#12** are subsumed by **D14**. Merging them — not done now, to preserve the
build-up record — would take the doc from 15 decisions to ~10 cleaner ones.

## License

Baking AGPL-3.0 Dendrite (and the mautrix/gomuks stack) into the binary makes the
combined, distributed work AGPL — including the network-use clause. **arbos
relicenses MIT → AGPL-3.0.** Consequences to accept up front: the `curl | bash`
binary must carry a written offer of corresponding source (trivial — it is open
source already); downstream embedders inherit AGPL; the network clause obliges
offering source to users who interact over the network (arbos.life included).
This is a deliberate, project-level choice, not an implementation detail, and it
is the precondition for the whole "baked-in, single binary" design.

## Seamlessness (no flag day)

- **No offline/online fork to get wrong.** The homeserver is always local, so
  storage is never "absent"; solo, offline, and networked are the *same* path
  with different membership and reachability. The LLM `fake` tier (no API key)
  is orthogonal and unchanged — that is about the model, not the store.
- **Old logs decode unchanged.** `Audience` is additive with `AudienceLocal`
  zero value (ADR-0010): no version bump, no upcaster.
- **Migration is a projection, not a rewrite.** An existing SQLite session is
  imported by replaying its `Room`-audience events into a freshly created local
  room; the `Local` overlay stays put. The room is just another event log, so
  the import is a fold.

## Bootstrapping: `arbos .` just works (first-run lifecycle)

The substrate must be invisible on first run: `arbos .` → you are *already*
chatting, same room every time, no login, no "create a room." The rule is
**deterministic + idempotent + lazy**:

- **Silently ensure (never prompt), idempotently:** the embedded homeserver (keys
  from the device key, D3), the **agent user** `@<agentID>:<machine>`, an
  **anonymous local human user** (no login — the web/TUI act as it), and **the
  dir's root room**. The root room's alias is **derived from the AgentID/dir key**,
  so re-running `arbos .` *rejoins the same room* — same dir, same conversation,
  every run (the ADR-0035 stable-URL UX, now a room). Power levels are set once at
  creation: the human is **owner (PL 100)**, the agent a member. The user never
  sees "power level" — it is just "your room, your agent."
- **Lazy, never eager:** the per-agent **Space** materializes only when a *second*
  room exists (no empty folder to manage); the **`@const`↔agent DM** (the
  out-of-band notice target, #1) materializes only when the *first* notice needs
  delivering.
- **No login on the golden path; upgrade in place on link:** the anonymous local
  human is the operator until linking (P4), which **upgrades it to `@const` on the
  commons** with the device key proving continuity (ADR-0033) — no lost rooms.

(Rejected: eager scaffolding — clutter; prompting to create a room/account —
breaks no-signup; a random room id — loses "same dir, same conversation.")

## Phasing (sequenced for option value — subtract, then scaffold)

- **P0 (scaffold; every later phase needs it):** the `Audience` field + the
  `ProjectEvent`-vs-`Audience` split; the one-root-key derivation (D3); confirm
  `SessionID`-as-room-id needs no core change. Pure data-structure work.
- **P1 (the substrate):** embed Dendrite on loopback, **one per machine** (D8),
  with the device key as its signing root (D3); the mautrix-go `SessionStore`
  adapter; dir-agents as users; a *local* person user. Every session is a room on
  *your own* homeserver. No federation yet — proves the swap at zero federation
  risk, and already deletes the dual store.
- **P2:** humans + multiple agents in one local room via membership + power
  levels + `sender→Grant`. *Subtract* the bespoke share machinery — including the
  **People surface** (`PeopleSurface`/`PeopleChat`, its scoped seam, the `people`
  surface kind, and the roster/typing/`chatNote` seam frames) and
  `ShareChat`/`ShareDialog`: the human side-chat renders inline as
  `chat_note`s, the roster becomes the member list, and "share" becomes "invite."
- **P3:** federation over the forest relay; the `server_name` (machine) ⟂
  web-entry (per-dir) split (D8); cross-node rooms; remote delegation as
  federated `task`/`kernel_event` — unifying local and remote delegation.
  *Subtract* the control-seam codec.
- **P4:** the person identity `@const` homed on the always-on commons + the
  link/cross-signing/key-backup flow and own-agent auto-trust (D9); arbos.life as
  a big node of the same binary (large rooms); directory as Spaces. *Subtract*
  the forest identity broker, shrinking the head to DNS + TLS + relay. Adopt a
  mautrix bridge; *subtract* `internal/messenger`.
- **P5:** multi-agent working rooms (D13 — addressed-only activation, dual
  budget, hop-counter loops); wallet/attestation as signed events; agent-to-agent
  payments across federation (ADR-0036 P4).

## Amendments to existing ADRs

- **ADR-0033:** the account backend *shrinks* — identity, directory, and
  presence move into the substrate (your homeserver, Spaces, Matrix presence);
  the account keeps only LLM routing/billing (the `Endpoint`) and the secret
  vault. **"One account per device" becomes "one homeserver per machine"** (D8):
  the anonymous device account (P1) is the machine/homeserver identity, and
  **linking (P2) attaches a separate human user `@const` homed on an always-on
  server** (D9) — a new, explicit, device-independent identity. The device key
  gains the (derived) homeserver-signing and agent-cross-signing roles (D3).
- **ADR-0034:** the forest head *shrinks* from identity-broker + directory +
  relay to **DNS/subdomain lease + TLS + dumb relay**; the relay is now
  *literally* transport (its stated ideal). The control seam's remote-agent role
  is superseded by federation; the "anonymous vs linked name" tiers become
  homeserver naming tiers. The **lease governs URL liveness, not federation
  delivery** (peers retry against the stable `server_name`; Topology/#9), and an
  **optional always-on room mirror** is a linked-tier reliability feature.
- **ADR-0035:** the per-(dir,machine) name **splits** (D8): the `server_name` is
  per-machine (device-key-derived), while the deterministic per-dir name becomes
  a **web client entry point and an agent *user* localpart**, not a server —
  **web-URL ⟂ `server_name`.** "More than one agent = more than one dir" becomes
  more users on the machine homeserver; per-dir homeserver isolation is opt-in.
  The tool denylist guards the machine root.
- **ADR-0036:** attestations and payments are signed Matrix events; the
  **human (`@const`) receive/attestation identity is now distinct from each
  agent's** (D9), so "who gets paid / who attested" differentiates person from
  agent. P4 (agent-to-agent payments) gets its transport for free.
- **ADR-0013:** local and remote delegation unify into "invite a user to a
  room"; the `LLMAgent`/`ArbosAgent`-local/`ArbosAgent`-remote transport split
  collapses to "same homeserver vs federated"; a child is a thread or child room
  (D11).
- **ADR-0027:** fork = a **new room** (Space child) seeded by re-emission, still
  rebinding the client to the successor (D11); the per-prefix copy is
  `life.arbos.imported` events.
- **ADR-0037:** a branch is a **thread anchored to the highlighted event** with a
  scoped projection, not a separate room (D11); accept/discard inject the summary
  into the parent room via the unchanged ADR-0015 context machinery.
- **ADR-0038:** `Author`/`chat_note`/projection-exclusion are subsumed by
  `sender` and the `Audience`×projection 2×2.
- **ADR-0016:** gains an **outbound twin** (D12) — the same "policy at the
  boundary" choke point, applied to the Local→Room raise: the room is a
  destination secrets are never bound to, and a redaction/classification seam
  guards content that ADR-0016's broker never managed (e.g. `cat .env`). The
  vault itself moves to **Matrix SSSS** (D14): encrypted account data the broker
  resolves at the boundary; the per-node file becomes the air-gapped fallback.
- **ADR-0019:** the *reservation* of `Session.Principal`/`Origin` is **consumed
  and collapsed** (D15a): `Principal` becomes the Matrix `sender`, `Origin` is
  removed (the homeserver is in `sender`; the door is a bridge user/room).
- **License:** supersedes the MIT `LICENSE` with AGPL-3.0.

## Risks (stated, not hidden)

- **Dendrite is in maintenance mode** and not built for large rooms or HA. It
  carries the commons' big-room ceiling and some federation-feature gaps.
  Mitigation: it is Go and "easy to hack" (fork/patch), the client speaks plain
  C-S so a node could fall back to a loopback Conduwuit subprocess by changing a
  base URL (losing only in-process embedding), and big public rooms can be
  deferred.
- **AGPL** narrows downstream reuse and obliges network-source offers. Accepted.
- **A homeserver per node is heavier than a client** (DB, federation state,
  crypto, key generation on first run). Mitigation: Dendrite's embedded mode
  (SQLite, in-memory NATS, monolith) targets exactly small/P2P nodes.

## Alternatives rejected

- **Client of a central arbos.life homeserver (the earlier P1).** Reintroduces
  the client-vs-peer asymmetry, a central dependency, a base-URL swap, and the
  hicli local-cache layer — all of which the baked-in peer model *deletes*. It
  was the lighter start, but it is the wrong day-one shape.
- **Two hats (own homeserver for private, commons account for crowds).**
  Asymmetric by construction; "same binary, uneven hardware" gives the same
  outcome with one model instead of two.
- **Bolt Matrix on as the Nth messenger door.** Leaves the SQLite log, share
  mechanism, forest protocol, and control seam standing in parallel — the
  opposite of the subtraction this design exists for.
- **Room as pure transport, kernel session stays canonical.** Two event logs
  forever plus a sync/dedup layer; loses D1's "compaction is a local overlay on
  immutable shared history."
- **Federate everything (no `Audience`).** Leaks reasoning/usage/memory to peers
  and the homeserver; D1 makes federation visibility first-class instead.
- **Subprocess homeserver (Conduwuit/Synapse) instead of embedded Dendrite.**
  Keeps MIT and dodges Dendrite's maturity risk, but forfeits "baked-in, single
  binary" — the explicit requirement this revision is built around.
- **A new mesh protocol (finish the forest's own wire).** Exactly what we are
  deleting; Matrix is a decade of solved federation, attribution, and moderation.
