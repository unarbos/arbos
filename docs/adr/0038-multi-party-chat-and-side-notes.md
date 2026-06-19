# ADR-0038 — Multi-party chat: attributed prompts and human-to-human side notes

- Status: Accepted
- Date: 2026-06-19
- Refines: ADR-0034 (gateway auth and the forest), ADR-0008 (sealed event sum)

## Context

Sharing a session turned arbos from a single operator talking to an agent into a
room: several people on the same conversation, reached through different doors
(the host's web tab, a share guest's browser, a Telegram bridge). Two needs fall
out of that and neither existed before.

First, **attribution**. When two humans prompt the same agent, the agent — and
every other participant's screen — must be able to tell them apart. A user
message can no longer be assumed to come from "the operator"; it needs the name
of the human who sent it. That name is self-asserted (a guest types it on the
invite), so it must be established at a place the sender cannot forge.

Second, **a back-channel**. Collaborators want to talk to *each other* —
"should we ask it to use Postgres instead?" — without that line becoming a turn,
entering the model's context, or polluting the trajectory. There was no event
for human-to-human speech: every message in the log was something the model
would eventually see.

Both are kernel-shaped: a new field on the persisted message, and a new
persisted event kind. Costly to reverse, so they get an ADR (ADR-0001).

## Decision

**One identity plane, established at the door, used two ways: attribution on
prompts, and a side-chat event that reaches people but never the model.**

### Attribution: a server-stamped Author on the message

`PromptIntent` and `SteerIntent` gain an `Author` field — the self-asserted
display name of the human who sent the line — which lands on the user
`Message.Author` and rides the `Queued` kernel event so other doors render the
sender's name. The rule that makes it trustworthy:

**Author is stamped server-side from the connection's identity, never trusted
from the client frame.** A share guest picks a display name once, on the invite
(`sanitizeGuestName`, defaulting to `"Guest"`); the gateway binds it to their
authenticated principal (ADR-0034) and overwrites any `author` the guest puts in
a frame. `filterShareFrame` enforces this on every share connection — a guest
who spoofs `"Host"` is rewritten to their real bound name. The local operator
and single-party sessions leave `Author` empty, so the common case is unchanged
and nothing renders a redundant name.

### The side chat: a logged event that is excluded from projection

A new sealed payload `core.ChatNotePayload` (kind `chat_note`,
`CurrentEventVersion` unchanged — additive, ADR-0010) carries a human-to-human
side-chat line. It reuses `Message` for the author and attachment apparatus;
`Role` is `RoleUser` for shape only, and `Kind()` returns `chat_note`
*regardless of role*, so `Event.Validate`'s user/assistant role check does not
apply and the side chat is never mistaken for a turn.

The load-bearing decision: **`chat_note` is logged but excluded from
`ProjectEvent`** (returns `(Message{}, false)`) alongside the other
non-conversational kinds. Doing it at the single event→Message mapping home
keeps it out of *every* projection at once — the live prompt, replay, compaction
folding (ADR-0014), and the recall tool. So a side note:

- **replays and interleaves** by `Seq` like any event, so a participant who
  joins late sees the back-channel in order; but
- **never enters the model context** and never appears in exported trajectories
  as model input.

It reaches people, not the model. That is the whole point, and projection
exclusion is the one place that guarantees it.

### The live presentation is its own kernel event

`ChatNoteIntent` is the intent a door sends; the actor appends a
`ChatNotePayload` to the log, broadcasts a `ChatNote` kernel event to every
door, and **never starts a turn**. `ChatNote` is deliberately a *separate*
kernel-event type from a queued prompt: reusing the prompt event would make
every door's UI think a turn is in flight. It carries `Author` (server-stamped
as above) and `Origin` (the per-connection door marker) so the existing
cross-door echo + self-suppression logic applies verbatim — the sender's own
door does not double-render the line it just sent, while every other door shows
it labeled by name and ordered by timestamp.

### Permissions ride the existing share grants

Posting to the side chat requires write permission. A read-only guest's
`chat_note` frame is dropped by `filterShareFrame` before it reaches the actor
(it is not a turn, but it is still a write to the shared log). Write guests post
under their bound name. This reuses ADR-0034's permission model rather than
inventing a side-chat-specific one.

## Consequences

- One new persisted field (`Message.Author`) and one new event kind
  (`chat_note`), both additive: no schema bump, no upcast rule, old logs decode
  unchanged (ADR-0010). The codec and the `exhaustive` projection switch each
  gain one case.
- Attribution is correct by construction: because `Author` is stamped from the
  authenticated principal and overwritten on every share frame, a guest cannot
  impersonate the host or another guest. Spoofing is a tested invariant.
- The side chat costs the model nothing: excluded from projection, it never
  enters the context window, never inflates token usage, and never appears as
  training input — a true out-of-band channel that still has a durable, ordered,
  replayable home in the log.
- A late joiner reconstructs both the conversation and the back-channel from the
  one log, in `Seq` order, with no separate store.
- The system prompt was taught to use the speaker label for identity, so the
  agent addresses each participant by their own name in a multi-party room.

## Alternatives rejected

- *Trust the Author the client sends* — lets any guest impersonate the host.
  Server-side stamping from the authenticated principal is the only safe place;
  the client field is overwritten, never read.
- *Make the side chat a normal message excluded by a flag the model is asked to
  ignore* — the model would still see it (tokens, trajectory, possible
  leakage). Excluding it at projection is a structural guarantee, not a request.
- *Reuse the queued-prompt kernel event for side-chat lines* — every door would
  read it as "a turn is starting." A distinct `ChatNote` event keeps the
  in-flight-turn signal honest.
- *A separate store or table for the back-channel* — fragments the single
  source of truth, breaks `Seq` interleaving, and duplicates replay. One log,
  one ordering, projection decides who sees what.
- *A side-chat-specific permission* — the existing read/write share grant
  already expresses "may write to this session"; dropping read-guest notes in
  `filterShareFrame` reuses it.
