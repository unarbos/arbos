# Polls in chats

Status: **Proposed design** (2026-11-11). Nothing here is built. For const to approve before any code.

Companion docs: [`docs/crdt-sync.md`](crdt-sync.md) (chat-doc schema, reactions), and the proposed
`docs/matrix-groups-e2ee-spec.md` (branch `cursor/matrix-groups-e2ee-spec-59c0`, not on `main`), which
moves DMs and groups onto Matrix.

---

## 0. Decision in one paragraph

A poll is **an ordinary message with one `poll` part** and a text fallback. A vote is **one small
Matrix event per vote** that points at the poll message; the tally is computed by every reader from
those events. Polls ship on **Matrix rooms only**: server channels now, DMs and groups when they cut
over. We do not build a second vote store in Loro, and we do not use Matrix's native `m.poll.*` event
for the poll itself.

---

## 1. What was asked, and the smallest thing that answers it

People in the community chat asked to put a question to the room and see what the room thinks.
Today the only way is to type the options and count emoji reactions by eye.

**v1:**

- One question, 2–10 fixed options.
- Single choice by default; one toggle "Allow multiple answers".
- Votes are visible: counts on the card, names behind a tap (same as reaction detail).
- You can change or retract your vote until the poll closes.
- The author, or an owner/admin of the chat, can close it. Closed is final.

**Not in v1, on purpose:**

- Anonymous polls. In a plaintext room the homeserver and relay see every vote, so "anonymous"
  would be a UI promise the system does not keep.
- Quizzes (a correct answer), scheduled close, adding options after posting, editing the question.
- Reopening a closed poll. Per-vote notifications. Exporting results.
- Polls in 1:1 agent project chats: `ask_user` with a `choice` widget already covers that
  (`crates/arbos-core/src/widget.rs`).

---

## 2. Where things stand today (read from the code)

| | People chats (DMs, groups) | Server channels |
| --- | --- | --- |
| Store | Loro chat doc per room, relay stores opaque rows (`docs/crdt-sync.md` §2, §6.2) | Matrix room, no Loro doc (`crates/arbos-relay/src/matrix/servers.rs`) |
| Message | `messages[]` map with `parts[]` (`crates/arbos-core/src/doc.rs`) | `m.room.message`; `body` becomes a `text` part, `life.arbos.parts` is appended (`crates/arbos-matrix/src/rows.rs::message_row`) |
| Reaction | Flat keys `"emoji\|sender" → true` under `reactions`, written by `ChatDoc::set_reaction`, projected by `project_reactions` | None. `message_row` writes `"reactions": {}`; `client.rs` has no reaction, edit or redaction handling |
| Who checks the writer | `crates/arbos-relay/src/push_guard.rs` checks message insert, delete and `sender`/`role` rewrites. A push that "touches no message" (a reaction) is allowed unchecked | The homeserver stamps `sender` on every event |
| Mirror | One-way to Matrix as `m.room.message` + `life.arbos.parts` (`crates/arbos-relay/src/matrix/worker.rs::message`) | n/a |
| Agents | Box reads the Loro doc (`crates/arbos-agentd/src/people.rs`); outside agents get rows from the relay (`crates/arbos-relay/src/outside.rs`). Text comes from `text` parts only (`MessageView::text`, `message_text`) | No agents. `arbos-agentd` has no Matrix code |
| Push | `notify::announce` | `matrix/reads.rs::on_notify`, which skips any event that is not `m.room.message` |

Matrix polls (MSC3381) in the pinned dependencies (`Cargo.lock`: `matrix-sdk 0.19.1`,
`ruma 0.17.1`, `ruma-events 0.35.1`):

- `ruma-events` has `m.poll.start` / `m.poll.response` / `m.poll.end` and the unstable
  `org.matrix.msc3381.poll.*` twins, all behind the `unstable-msc3381` feature (`src/lib.rs:179`,
  `src/enums.rs`).
- `matrix-sdk-base 0.19.1` turns that feature on unconditionally in its own `ruma` dependency, so the
  types and `poll::compile_poll_results` are **already compiled into our build**. `arbos-matrix`
  would need no new feature flag (`crates/arbos-matrix/Cargo.toml` uses `default-features = false`,
  which does not affect this).
- `ruma`'s own doc comment says the stable types are for room versions with extensible events and
  none exist yet; the unstable types are what to send. `matrix-sdk`'s edit path
  (`src/room/edit.rs`) also uses the unstable types.
- `matrix-sdk-ui` (the timeline that aggregates polls for you) is **not** in `Cargo.lock`.

---

## 3. Options weighed

**(a) Native Matrix poll events.** Types are free (above). But the poll would be an event that is
not `m.room.message`, and everything we have keys on that type: `message_row` returns `None`, the
sync handler is typed to `OriginalSyncRoomMessageEvent` (`client.rs:140`), the channel push path
skips it, the relay mirror only writes it, agents only read `text` parts. Every build already
installed would show **nothing** where the poll is. What native buys is interop with other Matrix
clients, and the crate header is explicit that no app talks Matrix on its own and users never see
the word. Rejected for the poll message. Its rules are kept (section 5).

**(b) The relay's own model in Loro.** Same `poll` part in `parts[]`, votes as a flat map on the
message (like `reactions`), core writers in `doc.rs`. Works in today's groups. Costs: `push_guard`
does not look inside a message's nested maps, so any member could write another member's vote; a
real poll needs a new ownership check in the relay's most delicate module. And it is throwaway: the
E2EE spec retires Loro rooms for people chats, so votes would need a one-off backfill into Matrix.
Rejected as the default; kept as the fallback in open decision 1.

**(c) A poll is a message whose options are reactions.** Zero schema change, and in Loro groups
people can already do it by hand. It cannot enforce single choice, cannot close, anyone can add a
stray emoji, and the sender in a Loro reaction key is unchecked. On Matrix it is worse for privacy:
a reaction's emoji lives in `m.relates_to`, which `matrix-sdk-crypto` copies outside the ciphertext
(`olm/group_sessions/outbound.rs:594`), so the server reads every vote even in an encrypted room.
Rejected.

**Recommended: poll part on a normal message, votes as events, Matrix rooms only.**

- The poll rides the path every message already takes. Old builds show the fallback text. Push,
  channel-list preview, mirror and agent context need no change to show the question and options.
- Vote authorship is the homeserver's `sender`. No new guard code.
- One row shape for Mac, iOS and agents: the reader folds votes into the `poll` part.

What E2EE means for it:

| Room | Who can read votes | What the server can tally |
| --- | --- | --- |
| Channel, large group (plaintext) | Members, homeserver, relay | Everything. v1 still tallies in the reader |
| DM, group ≤ 200 (encrypted, proposed) | Member devices only. The poll and each vote are `m.room.encrypted` | Nothing. It sees who sent an event and when, not that it was a vote or what for |
| Encrypted room with an agent (spec "Mode A", proposed) | Members, plus the relay appservice holding the agent's key | The relay can, for the agent's context |

To keep the middle row true, a vote names its poll **inside** the content, not in `m.relates_to`.

---

## 4. Data shape

Poll message: `m.room.message`, `body` = text fallback, `life.arbos.id` = the app's ULID, and

```json
"life.arbos.parts": [
  { "id": "<ulid>", "kind": "poll", "question": "Where do we meet?",
    "options": [ { "id": "a", "label": "Lisbon" }, { "id": "b", "label": "Berlin" } ],
    "max": 1 }
]
```

`options` reuses `ChoiceOption { id, label }` from `widget.rs`. `max` is `1` (single choice) or
`options.len()` (multiple).

Vote: event type `life.arbos.poll.vote`, content `{ "poll": "$event_id", "selections": ["a"] }`.
Empty `selections` retracts. Close: `life.arbos.poll.close`, content `{ "poll": "$event_id" }`.

What the apps receive: the same `poll` part with `votes: { "a": ["user:gh_1", …] }`, `closed: bool`
and `mine: ["a"]` folded in by `arbos-matrix`.

---

## 5. Rules

**Bounds** (one validator in `arbos-core`, used at every boundary below):

- `question`: 1–300 characters after trim. `options`: 2–10. `label`: 1–100 characters after trim.
- Option `id`: `^[a-z0-9]{1,8}$`, unique within the poll. `max`: `1..=options.len()`.
- Unknown fields are rejected (`deny_unknown_fields`) on the part, the vote and the close.
- One `poll` part per message, and nothing else beside the text fallback.
- A vote's `selections`: each a known option id, no repeats, at most `max`.

**Where it is checked:**

1. Composer, before send (Mac, iOS).
2. `arbos-matrix`, on every event read. The proxy (`/_matrix/client/*`) forwards content as-is and
   cannot read encrypted rooms at all, so the reader is the real boundary. Today `message_row`
   appends `life.arbos.parts` without looking; the `poll` part must be parsed strictly. An invalid
   poll part is dropped and the row shows its fallback text. An invalid vote or close is ignored as
   if never sent.
3. Relay, when it sends a poll or vote for an agent (phase 4), and when it renders one to text.

**Tally** (a pure function of the set of events, so arrival order does not matter):

- One vote per user: the latest valid vote per **Matrix `sender`** wins, ordered by
  `origin_server_ts` then event id. Never key on `life.arbos.sender`: it is a content field.
- Duplicates collapse on event id. A vote that arrives before its poll is held until the poll loads.
- Close counts only from the poll's author or a member at power level ≥ 50 (`ADMIN_PL` in
  `matrix/spaces.rs`; group owners are promoted in `worker.rs::create_room`). The first valid close
  wins. Votes stamped after it are ignored.
- A voter who leaves or is removed keeps their vote, like their messages stay. Non-members cannot
  vote: the homeserver refuses the send.
- No edits. A typo is fixed by deleting the poll and posting a new one.
- Poll message deleted: the row goes, its votes point at nothing and are ignored.

**Restricted channels.** A restricted channel sets `events_default: 50` (`matrix/spaces.rs`), which
would stop members voting on an admin's poll. The relay adds `"events": { "life.arbos.poll.vote": 0 }`
to channel power levels at create and in `reconcile_posting`.

---

## 6. How it surfaces

**Text form** (one function in `arbos-core`; used for the fallback `body`, push text and agents):

```text
Poll: Where do we meet?
1. Lisbon — 3 (Ana, Ben, Cy)
2. Berlin — 1 (Di)
Open · 4 voted · pick one
```

The fallback written at creation is the first lines without counts.

**iPhone.** `Part` in `apps/ios/Arbos/Arbos/Model/Model.swift` gains `.poll`; today it would decode
as `.unknown`. A poll card in `UI/Transcript.swift`, built from the option-list look of
`UI/QuestionCard.swift`. Create from the composer's attach entry in `UI/ChatView.swift`. Send, vote
and close go through `Store/ServerStore.swift` to three new verbs in `crates/arbos-ffi/src/matrix.rs`.

**Mac.** `Part` in `crates/arbos-mac/src/model.rs` gains a poll variant (today `Part::Unknown`).
The card reuses `card_frame` / `option_list` / `option_row` from `ui/widgets.rs`, drawn from
`ui/transcript.rs`. Create from the `+` menu in `ui/composer.rs`. Calls sit next to `send_text` in
`sync/relay.rs`.

**Rust.** New `crates/arbos-core/src/poll.rs` (shape, validate, tally, text). `arbos-matrix` gains
`send_poll`, `vote`, `close_poll`, handlers for the two new event types, and the fold in `rows.rs`;
it would newly depend on `arbos-core`.

**Agents.** Reading costs nothing: the fallback is a `text` part, which is what every agent path
already reads. A live tally reaches agents only when the relay renders it, which needs the relay to
see the room (phase 4). Creating and voting are two tools, added to `crates/arbos-agentd/src/mcp.rs`
and `crates/arbos-sidecar/src/mcp.rs`, sent by the relay as the agent's Matrix user.

**Notifications.** A new poll notifies like any message, with the fallback as its text. Votes and
closes do not notify and do not raise unread.

---

## 7. Phases

1. **Shape and rules.** `arbos-core/src/poll.rs` only.
   Verify: `cargo test -p arbos-core poll` — bounds, unknown fields, latest vote wins, retract,
   vote after close, close by a non-author, and the same tally from shuffled event order.
2. **Read path.** `arbos-matrix` parses the part and folds votes; the relay opens the vote event in
   channel power levels.
   Verify: `rows.rs` tests from event JSON; a member votes in a restricted channel in a relay test;
   the current shipped build shows the fallback text for a poll posted by hand.
3. **Create, vote, close in server channels.** FFI verbs, Mac card, iPhone card.
   Verify: two accounts in one test server on Mac and iPhone: create, vote, change, retract, close,
   relaunch and see the same result. One UI test per app in the style of `ReactionToggleTests`.
4. **DMs and groups, and agents.** Rides the Matrix cutover of people chats; the poll code is the
   same. Adds the relay's text rendering for agent context and the two agent tools.
   Verify: an agent in a test group answers "what is winning?" correctly and can vote once. In an
   encrypted room, Synapse `/messages` holds only `m.room.encrypted` for the poll and its votes, and
   a vote raises neither a banner nor the unread count on another device.

---

## 8. Open decisions for const

1. **Wait for the cutover, or build for today's groups?** Default: wait. Channels get polls first;
   groups and DMs get them when they move to Matrix. The alternative is option (b): a Loro vote map
   plus a vote-ownership check in `push_guard`, and a backfill later.
2. **Multiple answers in v1?** Default: yes, as one toggle. The tally rule is the same.
3. **Who may close?** Default: the author and the chat's owner/admins. Alternative: author only.
4. **Who may create a poll?** Default: anyone who can post in the chat. Alternative: owner/admins
   only in servers.
5. **Voter names visible?** Default: yes, behind a tap. Alternative: counts only (the names would
   still be readable by anyone with the room's events).
6. **Option cap.** Default: 10. MSC3381 and `ruma` allow 20.

---

## 9. Not verified

- Which chat "the community chat" is. Assumed a people group on Loro (the "Arbonites" group appears
  in relay tests), which is why decision 1 matters.
- Push behaviour was read from the Matrix spec, not tested on our Synapse (`v1.163.0` in
  `scripts/deploy/matrix/docker-compose.yml`): that a custom event type does not notify in a
  plaintext room, and whether an encrypted vote notifies like any encrypted event. Phase 4 tests it.
- Per-event power level overrides on room version 12 (`homeserver.yaml.tmpl`): standard Matrix, not
  tried here.
- The E2EE spec and PR #2934 are proposed and not on `main`; their code was not read. Phase 4 and
  the encrypted rows of the table in section 3 depend on them landing as written.
- `matrix-sdk-ui`'s poll support was not read: the crate is not in our lockfile.
- Old-build behaviour (fallback text shown, poll part ignored) is from reading the `Part` decoders,
  not from running an old build.
