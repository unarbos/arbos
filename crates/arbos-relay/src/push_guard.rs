//! What a row pushed over the socket is allowed to do to a chat's messages.
//!
//! `rooms.rs` has always stored and forwarded opaque bytes: its `Push` arm checks size,
//! duplicate batch ids, the heal limit and the people gates, and nothing about content. Three
//! separate findings come out of that one gap, and they are checked here together because
//! their answers depend on each other:
//!
//! 1. **Deleting someone else's message.** `moderation.rs` is the one authorised path and it
//!    asks `message_authors` whose message it is, but a client could skip the route entirely
//!    and push a doc update that deletes the row — which is the bypass the module header of
//!    `moderation.rs` describes as the reason it exists.
//! 2. **Writing someone else's name on a message** (AF-599). `sender` is a client-written
//!    field, and readers downstream take standing and authority from it.
//! 3. **Claiming a push is a heal.** `PushHeader.heal` is a client-written bool that skipped
//!    `people::on_room_push` — the block, silenced, availability and rate-limit gates — while
//!    the row was still stored and broadcast. The *skip* is right: a heal carries other
//!    members' history, not a message of this device. Believing the flag is not. So the flag
//!    now only asks for the exemption, and [`PushCheck::Allow::carries_new_message`] decides
//!    whether the push has earned it.
//!
//! The three meet at `message_authors`: it is the relay's own record of who announced a
//! message, so it says whether an insert naming another member is history the relay already
//! attested (a real heal) or a name a client just made up (a forgery). That is why these are
//! one check and not three, and why the kind of a push is decided from what it contains rather
//! than from what it claims.
//!
//! A fourth rides the same diff: **polls** (`docs/polls.md`). A poll and each person's vote are
//! fields on the message precisely so that this module can see them — see [`check_polls`].
//!
//! Nothing here reads the pushed bytes for the *kind* of push, and that is deliberate. The
//! client authors its own CRDT ops, so anything derivable from them — op order, Lamport
//! clocks, which frontier they descend from — it can also shape at will. The relay's own
//! record of what it was told is the only part of the picture the pusher did not write.

use arbos_core::doc::{
    MessageChanges, POLL_CLOSED_FIELD, POLL_FIELD, POLL_VOTE_AT_PREFIX, POLL_VOTE_PREFIX,
};
use arbos_core::poll::{self, Poll};

use crate::auth::Principal;
use crate::moderation;
use crate::people;
use crate::state::AppState;

/// Fields that name who a message came from. A client writes them when it writes the message;
/// rewriting them afterwards on a row that already exists is what a forgery looks like.
const IDENTITY_FIELDS: [&str; 2] = ["sender", "role"];

/// The relay's own identity for the rows it writes (`notices::write_doc`). Those never come
/// through a socket, so they are never checked here; the constant is for tests that spell out
/// the distinction.
pub const SYSTEM_SENDER: &str = "system";

#[derive(Debug, PartialEq, Eq)]
pub enum PushCheck {
    Allow {
        /// The verified kind of this push: `true` when it carries a message of the pusher's
        /// own that the relay has no record of — a new message, which is what
        /// `people::on_room_push` exists to gate, whatever `PushHeader.heal` claimed.
        ///
        /// `false` for a push that only moves history the relay already attested, which is
        /// what a real heal is and why the exemption exists at all.
        carries_new_message: bool,
        /// `(message id, author)` for each message this push introduces that the relay has no
        /// author for yet, to record once the row is stored.
        ///
        /// Capturing it here is what makes the record authoritative rather than a claim. The
        /// announce routes also write it, but they are gated only on chat membership, take the
        /// message id from the request body, and never ask whether that message exists or who
        /// wrote it — so on an `INSERT OR IGNORE` the first announce to arrive becomes the
        /// author of record for any message that has none. The socket is the row's own
        /// authenticated arrival point, so attesting here removes the claim instead of
        /// narrowing the window it lands in.
        attributed: Vec<(String, String)>,
    },
    Refuse {
        code: &'static str,
        detail: String,
    },
}

/// Which names the socket may write under, in the spelling `message_authors` uses.
enum Pusher {
    /// A device is its person, and nobody else.
    Person(String),
    /// A box serves every agent its owner owns — `Principal::Box` names the owner, not one
    /// agent — so ownership is the test, exactly as `notify::sender_for` applies it on the
    /// announce path. The two paths agreeing matters: the record one writes is the record the
    /// other checks against.
    BoxOf(String),
}

impl Pusher {
    fn of(principal: &Principal) -> Option<Self> {
        match principal {
            Principal::Device { user_id, .. } => Some(Self::Person(format!("user:{user_id}"))),
            Principal::Box { user_id } => Some(Self::BoxOf(user_id.clone())),
            Principal::Admin => None,
        }
    }

    /// Is `sender` a name this socket may write under?
    fn owns(&self, state: &AppState, sender: &str) -> bool {
        match self {
            Self::Person(me) => sender == me,
            Self::BoxOf(user_id) => sender
                .strip_prefix("agent:")
                .and_then(|pid| state.db.project(pid).ok().flatten())
                .is_some_and(|p| &p.user_id == user_id),
        }
    }

    /// Is `sender` the mark of a row this socket wrote and the relay could never attribute?
    ///
    /// Exactly one thing writes the bare string `agent`: `ChatDoc::start_assistant_message`,
    /// which is how a box opens every assistant row. That spelling has no `agent:` prefix, so
    /// [`Self::owns`] matched nothing, [`Self::attribution`] returned `None`, and the relay
    /// recorded **no author at all** — after which it refused the box's own later removal of
    /// the row as `not_yours`, forever. On Jacob's box on 2026-11-04 one such removal was
    /// refused 231 times, and the reply queued behind it never arrived.
    ///
    /// This is deliberately a test on what the row says, not on the record being silent.
    /// "No record" would also match every row written before `message_authors` existed, and
    /// that set includes people's rows — letting a box delete those is the laundering this
    /// guard exists to prevent. The bare `agent` spelling cannot reach them: a person's row
    /// carries `user` or `user:<id>`, a notice carries `system`, and an attributed agent row
    /// carries `agent:<project_id>`. Nothing else in `doc.rs` writes bare `agent` outside
    /// tests and fixtures.
    ///
    /// It is also self-limiting. Boxes now sign their rows `agent:<project_id>`, so the rows
    /// this arm forgives are the ones already written by older boxes, and no new ones join.
    fn wrote_unattributed(&self, sender: Option<&str>) -> bool {
        matches!(self, Self::BoxOf(_)) && sender == Some("agent")
    }

    /// The author to record for a row this socket introduced, where the relay can name one.
    fn attribution(&self, state: &AppState, sender: Option<&str>) -> Option<String> {
        match self {
            // A person's row is that person's, named on the row or not.
            Self::Person(me) => Some(me.clone()),
            // A box writes for whichever of its owner's agents the row names. With no name on
            // the row there is nothing to resolve — `Principal::Box` is the owner, not one
            // agent — and a guess in this record is worse than a gap in it.
            Self::BoxOf(_) => sender.filter(|s| self.owns(state, s)).map(str::to_owned),
        }
    }
}

/// May this push do what its diff says it does?
///
/// `claims_heal` is `PushHeader.heal` as the client sent it, and it is used in one place only:
/// to decide whether to *record* the pusher as the author of a row wearing someone else's name.
/// It never grants anything — a claimed heal is still checked against the record, still refused
/// where the record contradicts it, and still gated the moment it carries a message of the
/// pusher's own. Setting the flag buys an attacker nothing but anonymity in a record that would
/// otherwise have named them.
pub fn check_push(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
    chat_id: &str,
    claims_heal: bool,
    changes: &MessageChanges,
) -> PushCheck {
    // Bytes the relay could not read tell it nothing about what the update does — but refusing
    // them is not available either. `rooms.rs` has always stored and forwarded opaque rows, so
    // a payload that is not a Loro update the relay's version can apply is within the contract,
    // and a loro format skew between a client and the relay would otherwise become a total
    // push outage rather than a missed check.
    //
    // So unreadable bytes keep their old route — stored, forwarded — and lose only what this
    // check could have granted them: no heal exemption, so they are gated like any new
    // message, and no authorship recorded, so they attest nothing.
    if changes.unreadable {
        return PushCheck::Allow {
            carries_new_message: true,
            attributed: Vec::new(),
        };
    }
    if changes.removed.is_empty() && changes.inserted.is_empty() && changes.rewritten.is_empty() {
        // Touches no message: a reaction, a command, a part edit, model settings. Nothing here
        // has an opinion about those, and none of them is a new message.
        return PushCheck::Allow {
            carries_new_message: false,
            attributed: Vec::new(),
        };
    }
    let Some(me) = Pusher::of(principal) else {
        // The admin, which reaches every route on the relay already. Nothing here would hold
        // it back, so it does not pretend to.
        return PushCheck::Allow {
            carries_new_message: false,
            attributed: Vec::new(),
        };
    };
    let author_of = |id: &str| state.db.message_author(chat_id, id).ok().flatten();
    // Resolved once, and only when something actually needs it: it reads the chat's roster.
    let moderates = || moderation::moderates(state, principal, project_id, chat_id);

    for (id, was) in &changes.removed {
        let mine = author_of(id).is_some_and(|a| me.owns(state, &a));
        if !mine && !me.wrote_unattributed(was.as_deref()) && !moderates() {
            return PushCheck::Refuse {
                code: "not_yours",
                detail: format!(
                    "{id} is not yours to delete; DELETE /room/{project_id}/{chat_id}/messages/{id} is the path that decides"
                ),
            };
        }
    }

    for (id, field) in &changes.rewritten {
        if IDENTITY_FIELDS.contains(&field.as_str()) {
            return PushCheck::Refuse {
                code: "not_yours",
                detail: format!(
                    "{field} on {id} was written when the message was; it is not rewritable"
                ),
            };
        }
    }

    if let Some(refusal) = check_polls(state, &me, chat_id, changes) {
        return refusal;
    }

    // A second row wearing an id the list already holds. Nothing legitimate does this: a
    // message id is a ulid, and a heal re-inserts a row the list is *missing*, so after a real
    // heal the id appears once. Two rows under one id is a client minting a new message under a
    // name the record has already given out.
    //
    // It has to be refused here rather than answered by its readers, because the record cannot
    // tell the two rows apart afterwards. `message_authors` is keyed on `(chat, message id)`
    // alone, so the author the relay attested for the first row answers for the second one too
    // — and every reader that resolves a message by id then reads the new row as the old row's
    // author: `moderation`, the "save this" hand-off, and `secrets::directing`, which takes who
    // is directing a turn from exactly this record.
    //
    // The pusher-attribution arms below do not catch it, because this push is not introducing a
    // message as far as the record is concerned: the id already has an author, so the
    // `(Some(a), s)` arm reads it as a heal of attested history and attests nothing new.
    for (id, _) in &changes.inserted {
        if changes.ids_after.iter().filter(|(x, _)| x == id).count() > 1 {
            return PushCheck::Refuse {
                code: "duplicate_id",
                detail: format!(
                    "{id} is already a message in this chat; a new message needs an id of its own"
                ),
            };
        }
    }

    let mut carries_new_message = false;
    let mut attributed = Vec::new();
    for (id, sender) in &changes.inserted {
        let mut attest = || {
            if let Some(who) = me.attribution(state, sender.as_deref()) {
                attributed.push((id.clone(), who));
            }
        };
        match (author_of(id).as_deref(), sender.as_deref()) {
            // History the relay attested to you: your own old rows, which is most of what a
            // device heals a room with.
            (Some(a), _) if me.owns(state, a) => {}
            // History the relay attested to someone else. A heal may carry it, but only under
            // the name the relay recorded. Dropping that name or changing it is the forgery,
            // whether the row then reads as another member or as nobody.
            (Some(a), s) => {
                if s != Some(a) {
                    return PushCheck::Refuse {
                        code: "not_yours",
                        detail: format!(
                            "{id} is {a}'s message; this push names it {}",
                            s.unwrap_or("nobody")
                        ),
                    };
                }
            }
            // No record, and no name or your own: a message the relay is seeing for the first
            // time. This is the push the people gates are for, and the moment to record whose
            // it is.
            (None, None) => {
                carries_new_message = true;
                attest();
            }
            (None, Some(s)) if me.owns(state, s) => {
                carries_new_message = true;
                attest();
            }
            // A system line. Every app writes these for call lines through
            // `ChatDoc::append_system_line`, which hard-codes `sender: "system"`, so refusing
            // them here would break Mac, iOS and agentd. Forged system lines are a separate
            // finding with its own fix in flight; this check neither closes nor worsens it.
            // The row is still attested to the socket that pushed it, which is the only
            // accountability a line nobody signed can have.
            (None, Some(SYSTEM_SENDER)) => attest(),
            // Another member's name on a message the relay has no record of (AF-599). This is
            // the one case the relay cannot settle by refusing. A chat doc is a CRDT with no
            // per-op authentication, so a row claiming `user:bob` and a row Bob wrote are the
            // same bytes — and the legitimate shape of "someone else's message arriving from
            // a third party" is a heal of history the relay lost. Refusing it would strand
            // exactly the history heals exist to carry.
            //
            // So the claim is let through and answered instead of blocked: outside a heal, the
            // relay records the *pusher* as the author, because whatever name the row wears,
            // this socket is who introduced it. A reader that resolves identity from the
            // record therefore sees the writer; only a reader that trusts the doc field sees
            // the claim, and closing that needs the posting model to carry a relay identity,
            // which is a separate change.
            (None, Some(_)) => {
                if !claims_heal {
                    carries_new_message = true;
                    if let Some(who) = me.attribution(state, None) {
                        attributed.push((id.clone(), who));
                    }
                }
            }
        }
    }
    // Reading "no record" as "new message" only holds where the record reaches. In a chat the
    // relay has never attested anything in, every message predates the record, so its silence
    // says nothing about any one of them — and treating that silence as proof of a new message
    // would refuse the heal of a room's whole history, which is the one case heals exist for.
    // Each announce narrows this, one chat at a time.
    if carries_new_message && !state.db.chat_has_authors(chat_id).unwrap_or(false) {
        carries_new_message = false;
    }
    PushCheck::Allow {
        carries_new_message,
        attributed,
    }
}

/// What a push may do to a poll: post one within bounds, and cast, change or retract the
/// pusher's own vote. Everything else is refused.
///
/// - The poll is written with its message and never again, so nobody edits a question after
///   people answered it or hangs a poll on a message that was not one.
/// - A vote is the field `vote|<voter>`, and the voter has to be a name this socket may write
///   under and a member of the chat. The name in the key is never taken on trust.
/// - One vote per person is the storage itself: one key each, last write wins.
/// - A vote's time (`voteAt|<voter>`) is the voter's to write like the vote, and a vote on a
///   timed poll has to carry one.
/// - The manual close (`pollClosed`) is the author's alone to write, is a time, and is never
///   taken back off.
///
/// What is *not* refused is anything an honest device can push in a race: a vote that reaches
/// the relay after the poll closed, a vote whose device clock is off, a second close from the
/// author's other device. A refused push is re-sent on every reconnect and nothing behind it
/// in that device's queue can go, so a refusal there would cut an honest device off the chat.
/// Those pushes are stored and then put right by the relay itself: see [`poll_corrections`].
///
/// The one allowance is the one the insert arms make: a row arriving whole that the relay has
/// already attested is history a heal carries, and it carries the votes on it. A device cannot
/// push its own vote and leave out everyone else's when it restores a row the relay lost.
fn check_polls(
    state: &AppState,
    me: &Pusher,
    chat_id: &str,
    changes: &MessageChanges,
) -> Option<PushCheck> {
    let refuse = |code, detail| Some(PushCheck::Refuse { code, detail });
    for (id, fields) in &changes.polls {
        let arrives = changes.inserted.iter().any(|(i, _)| i == id);
        let written: Vec<&str> = changes
            .rewritten
            .iter()
            .filter(|(i, _)| i == id)
            .map(|(_, field)| field.as_str())
            .collect();
        if written.contains(&POLL_FIELD) {
            return refuse(
                "poll_fixed",
                format!("the poll on {id} is written with its message; it is not editable"),
            );
        }
        let Some(poll) = fields.get(POLL_FIELD).and_then(Poll::parse) else {
            return refuse(
                "bad_poll",
                format!("{id} does not hold a poll within bounds"),
            );
        };
        let attested = arrives
            && state
                .db
                .message_author(chat_id, id)
                .ok()
                .flatten()
                .is_some();
        // Every vote on a row that arrives whole; on a row already here, the ones this push
        // wrote — including one it removed, which is why this walks the written fields and
        // not the fields that are left.
        let votes: Vec<&str> = if arrives {
            fields
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k.as_str())
                .collect()
        } else {
            written
        };
        for key in votes {
            if key == POLL_CLOSED_FIELD {
                if fields.get(key).and_then(poll::time).is_none() {
                    return refuse(
                        "bad_poll",
                        format!("the close on {id} is not a time, or was taken off"),
                    );
                }
                // The relay's record of who posted the row; the name on the row only where
                // the record does not reach.
                let author = state
                    .db
                    .message_author(chat_id, id)
                    .ok()
                    .flatten()
                    .or_else(|| {
                        let row = changes.ids_after.iter().find(|(i, _)| i == id);
                        row.and_then(|(_, sender)| sender.clone())
                    });
                if !attested && !author.is_some_and(|a| me.owns(state, &a)) {
                    return refuse(
                        "not_yours",
                        format!("the poll on {id} is its author's to close"),
                    );
                }
                continue;
            }
            let (voter, valid) = if let Some(voter) = key.strip_prefix(POLL_VOTE_PREFIX) {
                let timed = !poll.timed()
                    || fields
                        .get(format!("{POLL_VOTE_AT_PREFIX}{voter}"))
                        .and_then(poll::time)
                        .is_some();
                let allowed = fields
                    .get(key)
                    .is_some_and(|v| poll.selections(v).is_some());
                (voter, allowed && timed)
            } else if let Some(voter) = key.strip_prefix(POLL_VOTE_AT_PREFIX) {
                (voter, fields.get(key).and_then(poll::time).is_some())
            } else {
                continue;
            };
            if !valid {
                return refuse(
                    "bad_vote",
                    format!("the vote on {id} is not a choice that poll allows"),
                );
            }
            if attested {
                continue;
            }
            if !me.owns(state, voter) {
                return refuse(
                    "not_yours",
                    format!("the vote on {id} is {voter}'s to cast"),
                );
            }
            let member = people::load_chat(state, chat_id)
                .is_ok_and(|cm| cm.members.iter().any(|m| m.member == voter));
            if !member {
                return refuse(
                    "not_a_member",
                    format!("{voter} is not a member of the chat {id} is in"),
                );
            }
        }
    }
    None
}

/// How far a vote's or a close's time may sit from the relay's clock and be kept as written.
pub const POLL_CLOCK_TOLERANCE_MS: i64 = 2 * 60 * 1000;

/// Fields the relay writes back onto a poll row after it stored a push.
#[derive(Debug, PartialEq)]
pub struct PollFix {
    pub message_id: String,
    pub fields: Vec<(String, serde_json::Value)>,
}

/// The relay as the clock of a poll it can read. `now` is the relay's time. For each timed
/// poll a stored push wrote on, what the relay must write back so that the row every reader
/// reduces holds the relay's judgement and not the pusher's:
///
/// - **A close is final.** A `pollClosed` written over an earlier one is set back to the
///   earlier one. A first close keeps its time when it is within
///   [`POLL_CLOCK_TOLERANCE_MS`] before `now`, and is set to `now` otherwise, so it can
///   neither be dated back over votes already cast nor forward to keep the poll open.
/// - **No vote after the deadline.** Once the deadline that stood before the push has passed
///   by `now`, a changed `vote|x` or `voteAt|x` is set back to what it was (a first vote
///   becomes an empty one, timed `now`). Change, retraction and backdating alike.
/// - **A vote is timed by the relay.** While the poll is open, a `voteAt|x` further than the
///   tolerance from `now` — or a vote changed without a fresh time — is set to `now`.
///
/// These are corrections, not refusals, because honest devices produce every one of these
/// pushes (see [`check_polls`]). A row arriving whole that the relay attested before is a
/// heal restoring history: its votes, their times and its close are left as they come, since
/// stamping them `now` would void a finished poll's count. Someone able to delete a poll (its
/// author, a moderator) can therefore delete it and push it back under the same id with a
/// count of their own making; closing that needs a relay-side record of each poll.
///
/// Under end-to-end encryption the relay reads none of this and none of it holds.
pub fn poll_corrections(
    state: &AppState,
    chat_id: &str,
    changes: &MessageChanges,
    now: i64,
) -> Vec<PollFix> {
    let empty = serde_json::json!({});
    let mut fixes = Vec::new();
    for (id, after) in &changes.polls {
        let Some(poll) = after.get(POLL_FIELD).and_then(Poll::parse) else {
            continue;
        };
        let arrives = changes.inserted.iter().any(|(i, _)| i == id);
        let healed = arrives
            && state
                .db
                .message_author(chat_id, id)
                .ok()
                .flatten()
                .is_some();
        if !poll.timed() || healed {
            continue;
        }
        let before = changes
            .polls_before
            .iter()
            .find(|(i, _)| i == id)
            .map_or(&empty, |(_, fields)| fields);
        let time_of = |fields: &serde_json::Value, key: &str| fields.get(key).and_then(poll::time);
        let recent = |at: i64| (now - POLL_CLOCK_TOLERANCE_MS..=now).contains(&at);
        let mut fields = Vec::new();

        let closed_before = time_of(before, POLL_CLOSED_FIELD);
        match (closed_before, time_of(after, POLL_CLOSED_FIELD)) {
            (Some(was), is) if is != Some(was) => {
                fields.push((POLL_CLOSED_FIELD.to_owned(), was.into()));
            }
            (None, Some(is)) if !recent(is) => {
                fields.push((POLL_CLOSED_FIELD.to_owned(), now.into()));
            }
            _ => {}
        }

        let open = !poll::closed(poll.deadline(before.get(POLL_CLOSED_FIELD)), now);
        let voters: std::collections::BTreeSet<&str> = [after, before]
            .into_iter()
            .filter_map(|fields| fields.as_object())
            .flat_map(|fields| fields.keys())
            .filter_map(|key| {
                key.strip_prefix(POLL_VOTE_PREFIX)
                    .or_else(|| key.strip_prefix(POLL_VOTE_AT_PREFIX))
            })
            .collect();
        for voter in voters {
            let (vote, at) = (
                format!("{POLL_VOTE_PREFIX}{voter}"),
                format!("{POLL_VOTE_AT_PREFIX}{voter}"),
            );
            let vote_changed = after.get(&vote) != before.get(&vote);
            let at_changed = after.get(&at) != before.get(&at);
            if !vote_changed && !at_changed {
                continue;
            }
            if open {
                let fresh = at_changed
                    && time_of(after, &at).is_some_and(|t| {
                        (now - POLL_CLOCK_TOLERANCE_MS..=now + POLL_CLOCK_TOLERANCE_MS).contains(&t)
                    });
                if !fresh {
                    fields.push((at, now.into()));
                }
            } else {
                let was = before.get(&vote).cloned();
                fields.push((vote, was.unwrap_or_else(|| serde_json::json!([]))));
                let was = before.get(&at).cloned();
                fields.push((at, was.unwrap_or_else(|| now.into())));
            }
        }
        if !fields.is_empty() {
            fixes.push(PollFix {
                message_id: id.clone(),
                fields,
            });
        }
    }
    fixes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people::tests_support::{dm, person};
    use crate::people::PEOPLE_PROJECT;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn changes() -> MessageChanges {
        MessageChanges::default()
    }

    fn refused(c: &PushCheck) -> bool {
        matches!(c, PushCheck::Refuse { .. })
    }

    /// The verified kind of an allowed push, or `None` when it was refused.
    fn kind(c: &PushCheck) -> Option<bool> {
        match c {
            PushCheck::Allow {
                carries_new_message,
                ..
            } => Some(*carries_new_message),
            PushCheck::Refuse { .. } => None,
        }
    }

    /// A chat with Alice as owner and Bob as a plain member, and one announced message each.
    fn chat_with_two(state: &AppState) -> (String, String, String, Principal, Principal) {
        let (alice, alice_dev) = person(state, 101);
        let (bob, bob_dev) = person(state, 102);
        let chat = dm(state, &alice, &bob);
        state
            .db
            .add_member(
                &chat,
                &format!("user:{bob}"),
                "member",
                &alice,
                &serde_json::json!({}),
            )
            .unwrap();
        for (id, who) in [("m_alice", &alice), ("m_bob", &bob)] {
            state
                .db
                .insert_chat_event(&chat, None, id, Some(who), &format!("user:{who}"))
                .unwrap();
        }
        (chat, alice, bob, alice_dev, bob_dev)
    }

    /// The long path through this check. A member who writes a *new* id is recorded under their
    /// own name by the arms below, and that is what the earlier probe exercised. Re-using an id
    /// the chat already holds takes a different arm entirely: the id has an author already, so
    /// the push reads as a heal of attested history and attests nothing — leaving the owner's
    /// name in the record answering for a row the member wrote.
    ///
    /// `message_authors` is keyed on `(chat, message id)`, so every reader that resolves a
    /// message by id reads the new row as the old row's author, including
    /// `secrets::directing`, which takes who is directing a turn from this record.
    #[test]
    fn a_push_cannot_mint_a_second_message_under_an_id_the_chat_already_holds() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, _bob, alice_dev, bob_dev) = chat_with_two(&state);
        let as_alice = || {
            let mut c = changes();
            c.inserted = vec![("m_alice".to_owned(), Some(format!("user:{alice}")))];
            c
        };

        // Bob appends his own text under Alice's id and Alice's name. Both rows are in the list
        // afterwards, which is what says this is a new message rather than a heal.
        let mut c = as_alice();
        c.ids_after = vec![
            ("m_alice".to_owned(), None),
            ("m_bob".to_owned(), None),
            ("m_alice".to_owned(), None),
        ];
        assert!(refused(&check_push(
            &state,
            &bob_dev,
            PEOPLE_PROJECT,
            &chat,
            true,
            &c
        )));
        // Not claiming the heal does not help him either.
        assert!(refused(&check_push(
            &state,
            &bob_dev,
            PEOPLE_PROJECT,
            &chat,
            false,
            &c
        )));
        // Nor does the owner's own socket get to recycle an id: the record could not name
        // either row afterwards, whoever pushed them.
        assert!(refused(&check_push(
            &state,
            &alice_dev,
            PEOPLE_PROJECT,
            &chat,
            true,
            &c
        )));

        // The heal this must not break: Alice's row is *missing* from the list, so putting it
        // back leaves one of it. That is the shape every real heal has.
        let mut c = as_alice();
        c.ids_after = vec![("m_alice".to_owned(), None), ("m_bob".to_owned(), None)];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                true,
                &c
            )),
            Some(false),
            "re-inserting attested history under the name the relay recorded"
        );
    }

    /// Finding 1: the bypass. A push that deletes a message the relay attests to someone else
    /// is refused, which is what `moderation.rs` could not do from the HTTP route alone.
    #[test]
    fn a_push_cannot_delete_a_message_the_relay_attests_to_someone_else() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, bob, alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.removed = vec![("m_alice".to_owned(), Some(format!("user:{alice}")))];
        assert!(
            refused(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            "a member must not delete the owner's message by pushing a row"
        );

        // His own is his to delete, and the owner moderates anyone's.
        let mut c = changes();
        c.removed = vec![("m_bob".to_owned(), Some(format!("user:{bob}")))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(false)
        );
        let mut c = changes();
        c.removed = vec![("m_bob".to_owned(), Some(format!("user:{bob}")))];
        assert!(!refused(&check_push(
            &state,
            &alice_dev,
            PEOPLE_PROJECT,
            &chat,
            false,
            &c
        )));
    }

    /// Recovery, and the reason this arm exists.
    ///
    /// A box's own assistant row written by an older agentd carries the bare sender `agent`,
    /// which the relay could never attribute — so it holds no author for the row and used to
    /// refuse the box's own removal of it as `not_yours`. The box cannot skip a refused batch
    /// (Loro needs its ops gap-free), so it re-offered the same batch forever and everything
    /// written after the removal — the reply a person was waiting for — went with it. Jacob's
    /// box, 2026-11-04: 231 refusals, and the reply never arrived.
    ///
    /// Letting the box retire its own unattributable row is what lets that batch through, and
    /// with it every op queued behind it.
    #[test]
    fn a_box_may_retire_its_own_row_that_the_relay_could_never_attribute() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 301);
        let project = "proj_jam".to_owned();
        state
            .db
            .insert_project(&crate::db::Project {
                id: project.clone(),
                name: "Arbos".into(),
                icon: "R".into(),
                instructions: String::new(),
                models: serde_json::json!({}),
                state: "hibernated_light".into(),
                created_at: 1_700_000_000_000,
                last_active_at: 1_700_000_000_000,
                spend_cap_daily: 25.0,
                color: "#000".into(),
                user_id: alice.clone(),
                workspace_state: "stopped".into(),
                avatar: None,
                avatar_color: None,
                description: String::new(),
                primary_chat_id: None,
                avatar_color_index: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                image_key: None,
                avatar_version: 0,
            })
            .unwrap();
        let chat = format!("chat_{project}");
        let the_box = Principal::Box {
            user_id: alice.clone(),
        };

        // The removal the relay used to refuse, and the reply queued in the same batch.
        let mut c = changes();
        c.removed = vec![("m_wordless".to_owned(), Some("agent".to_owned()))];
        c.inserted = vec![("m_reply".to_owned(), Some(format!("agent:{project}")))];
        c.ids_after = vec![("m_reply".to_owned(), None)];
        assert!(
            !refused(&check_push(&state, &the_box, &project, &chat, false, &c)),
            "the reply is still stuck behind the box's own unattributable row"
        );

        // The limit of it. A person's row is not a box's to delete, whether or not the relay
        // ever recorded an author for it — and rows predating `message_authors` have none.
        for sender in [
            Some(format!("user:{alice}")),
            Some("system".to_owned()),
            None,
        ] {
            let mut c = changes();
            c.removed = vec![("m_theirs".to_owned(), sender.clone())];
            assert!(
                refused(&check_push(&state, &the_box, &project, &chat, false, &c)),
                "a box deleted a row sent as {sender:?}"
            );
        }

        // And it is the box's privilege, not everyone's: a person's device pushing the same
        // removal is refused exactly as before.
        let (_bob, bob_dev) = person(&state, 302);
        let mut c = changes();
        c.removed = vec![("m_wordless".to_owned(), Some("agent".to_owned()))];
        assert!(refused(&check_push(
            &state, &bob_dev, &project, &chat, false, &c
        )));
    }

    /// Finding 2 (AF-599): a made-up name is answered rather than blocked.
    ///
    /// Refusing it is not available. A chat doc is a CRDT with no per-op authentication, so at
    /// the relay a row claiming `user:alice` and a row Alice wrote are the same bytes, and the
    /// legitimate shape of "another member's message arriving from a third party" is a heal.
    /// What the relay can do is name the socket that introduced it, so every reader that
    /// resolves identity from the record sees Bob and not Alice.
    #[test]
    fn a_made_up_name_is_recorded_against_the_socket_that_pushed_it() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, bob, _alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.inserted = vec![("m_new".to_owned(), Some(format!("user:{alice}")))];
        let PushCheck::Allow {
            carries_new_message,
            attributed,
        } = check_push(&state, &bob_dev, PEOPLE_PROJECT, &chat, false, &c)
        else {
            panic!("refusing this would strand the history a heal carries");
        };
        assert_eq!(
            attributed,
            [("m_new".to_owned(), format!("user:{bob}"))],
            "the row wears Alice's name; the record names who pushed it"
        );
        assert!(
            carries_new_message,
            "outside a heal this is a new message of Bob's, whatever it calls itself"
        );

        // The same push claiming to be a heal records nothing rather than attributing the
        // history of a room to whoever healed it. That is all the flag buys: anonymity in a
        // record that would otherwise have named them.
        let PushCheck::Allow {
            carries_new_message,
            attributed,
        } = check_push(&state, &bob_dev, PEOPLE_PROJECT, &chat, true, &c)
        else {
            panic!("a heal carries other members' history by definition");
        };
        assert!(attributed.is_empty());
        assert!(!carries_new_message);

        // History the relay attested: a heal may carry it.
        let mut c = changes();
        c.inserted = vec![("m_alice".to_owned(), Some(format!("user:{alice}")))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(false),
            "re-pushing attested history is what a heal is"
        );
    }

    /// Finding 2, the other half: the author of a row that already exists is not rewritable,
    /// which is the move that turned the moderation route against itself.
    #[test]
    fn a_push_cannot_rewrite_the_author_of_an_existing_message() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, bob, _alice_dev, bob_dev) = chat_with_two(&state);

        for field in ["sender", "role"] {
            let mut c = changes();
            c.rewritten = vec![("m_alice".to_owned(), field.to_owned())];
            assert!(
                refused(&check_push(
                    &state,
                    &bob_dev,
                    PEOPLE_PROJECT,
                    &chat,
                    false,
                    &c
                )),
                "{field} must not be rewritable"
            );
        }
        // An ordinary field edit is none of this check's business.
        let mut c = changes();
        c.rewritten = vec![("m_bob".to_owned(), "preview".to_owned())];
        assert!(!refused(&check_push(
            &state,
            &bob_dev,
            PEOPLE_PROJECT,
            &chat,
            false,
            &c
        )));
        let _ = bob;
    }

    /// Finding 3: the kind of a push is decided from what it carries. A push with a message of
    /// the pusher's own is gated whatever the `heal` flag said; one that only carries attested
    /// history is not, which keeps the exemption a real heal depends on.
    #[test]
    fn the_kind_of_a_push_comes_from_its_contents_not_its_flag() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, bob, _alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.inserted = vec![("m_mine".to_owned(), Some(format!("user:{bob}")))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(true),
            "a message of your own is gated even when the push says heal"
        );

        // A row with no sender at all is still yours: nobody else could have written it.
        let mut c = changes();
        c.inserted = vec![("m_bare".to_owned(), None)];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(true)
        );

        // Pure attested history: a heal, and not gated.
        let mut c = changes();
        c.inserted = vec![("m_alice".to_owned(), Some(format!("user:{alice}")))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(false)
        );

        // Most of what a device heals a room with is its *own* old rows. The relay attested
        // them when they were announced, so they are history too — gating them is the Oct 23
        // outage, where a blocked member's heal was dropped.
        let mut c = changes();
        c.inserted = vec![("m_bob".to_owned(), Some(format!("user:{bob}")))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(false),
            "your own attested row is history, not a new message"
        );
    }

    /// The push names the author of the row it carries, so the record is made where the row
    /// arrives. Without this the record is still a claim: the announce routes take the message
    /// id from a request body, are gated only on chat membership, and insert first-write-wins —
    /// so any member could become the author of record for any message that had none.
    #[test]
    fn a_push_records_the_author_of_the_row_it_carries() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, bob, _alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.inserted = vec![
            ("m_named".to_owned(), Some(format!("user:{bob}"))),
            ("m_bare".to_owned(), None),
        ];
        let PushCheck::Allow { attributed, .. } =
            check_push(&state, &bob_dev, PEOPLE_PROJECT, &chat, false, &c)
        else {
            panic!("bob's own messages are his to push");
        };
        assert_eq!(
            attributed,
            [
                ("m_named".to_owned(), format!("user:{bob}")),
                ("m_bare".to_owned(), format!("user:{bob}")),
            ],
            "a row with no name on it is still the pushing device's person's"
        );

        // Attested history is already recorded, so a heal re-pushing it attests nothing new.
        let mut c = changes();
        c.inserted = vec![("m_bob".to_owned(), Some(format!("user:{bob}")))];
        let PushCheck::Allow { attributed, .. } =
            check_push(&state, &bob_dev, PEOPLE_PROJECT, &chat, false, &c)
        else {
            panic!("his own attested row is his to heal");
        };
        assert!(attributed.is_empty());
    }

    /// The property the push-side capture buys: once the row has arrived and named its author,
    /// a later announce naming someone else changes nothing. Before it, the announce was the
    /// only writer of this record and the first one to arrive won.
    #[test]
    fn an_announce_cannot_rename_an_author_the_push_already_recorded() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, bob, _alice_dev, _bob_dev) = chat_with_two(&state);

        state
            .db
            .record_message_author(&chat, "m_x", &format!("user:{bob}"))
            .unwrap();
        // Alice is in the chat, so this announce passes membership; the message id is hers to
        // put in the body. It must not take the row from Bob.
        state
            .db
            .insert_chat_event(&chat, None, "m_x", Some(&alice), &format!("user:{alice}"))
            .unwrap();
        assert_eq!(
            state.db.message_author(&chat, "m_x").unwrap(),
            Some(format!("user:{bob}"))
        );
    }

    /// Re-pushing someone else's attested row is a heal, but only under the name the relay
    /// recorded. Dropping the name leaves their words showing as nobody's, which is the same
    /// move as putting the wrong name on it.
    #[test]
    fn a_push_cannot_strip_the_name_off_someone_elses_message() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, _bob, _alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.inserted = vec![("m_alice".to_owned(), None)];
        assert!(refused(&check_push(
            &state,
            &bob_dev,
            PEOPLE_PROJECT,
            &chat,
            false,
            &c
        )));
    }

    /// The limit of what the record can say. In a chat the relay has attested nothing in,
    /// every message predates the record, so "no record of this message" is not evidence that
    /// the message is new — and reading it that way would gate the heal of a room's whole
    /// history, the one case heals exist for. The chat's first announce ends this.
    #[test]
    fn a_chat_the_record_does_not_reach_keeps_the_heal_exemption() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 201);
        let (bob, bob_dev) = person(&state, 202);
        let chat = dm(&state, &alice, &bob);

        let mut c = changes();
        c.inserted = vec![("m_old".to_owned(), Some(format!("user:{bob}")))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(false),
            "a chat with no attestations at all cannot judge one message in it"
        );

        // One announce, and the record speaks for this chat from here on.
        state
            .db
            .insert_chat_event(&chat, None, "m_1", Some(&bob), &format!("user:{bob}"))
            .unwrap();
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(true)
        );
    }

    /// Call lines. Every app writes them through `append_system_line`, which hard-codes
    /// `sender: "system"`, so this check has to let them through or it breaks the phone.
    #[test]
    fn a_client_written_system_line_still_gets_through() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, _bob, _alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.inserted = vec![("m_call".to_owned(), Some(SYSTEM_SENDER.to_owned()))];
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some(false)
        );
    }

    /// Bytes the relay cannot read are gated, not refused. `rooms.rs` stores and forwards
    /// opaque rows by contract — the room socket's own tests push `row-1` as a payload — so
    /// refusing an unimportable update would break that contract, and a loro format skew
    /// between a client and the relay would read as a total push outage. What such a push
    /// loses is only what this check could have granted: the heal exemption and a record.
    #[test]
    fn an_unreadable_diff_is_gated_rather_than_refused() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, _bob, _alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.unreadable = true;
        for claims_heal in [false, true] {
            let PushCheck::Allow {
                carries_new_message,
                attributed,
            } = check_push(&state, &bob_dev, PEOPLE_PROJECT, &chat, claims_heal, &c)
            else {
                panic!("opaque bytes are within the room socket's contract");
            };
            assert!(
                carries_new_message,
                "a push the relay cannot read does not get the heal exemption"
            );
            assert!(attributed.is_empty(), "nothing readable, nothing attested");
        }
    }

    /// A push that touches no message at all — a reaction, a command, a part edit — is not a
    /// message push and is nobody's business here.
    #[test]
    fn a_push_that_touches_no_message_is_allowed_and_ungated() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, _bob, _alice_dev, bob_dev) = chat_with_two(&state);

        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &changes()
            )),
            Some(false)
        );
    }

    // ---- polls -------------------------------------------------------------------------

    use arbos_core::doc::{ChatDoc, MessageIdentity, UserMessageOpts};
    use serde_json::json;

    /// A room doc as the relay holds it, and the diff of one push against it — the real
    /// `messages_changed`, so these tests cover the stored shape and the check together.
    struct Room {
        doc: ChatDoc,
        ids: Vec<MessageIdentity>,
    }

    impl Room {
        fn new() -> Self {
            Self {
                doc: ChatDoc::new(Some(1), false),
                ids: Vec::new(),
            }
        }

        fn push(&mut self, write: impl FnOnce(&ChatDoc)) -> MessageChanges {
            let before = self.doc.frontiers();
            self.doc.transact(write);
            let changes = self
                .doc
                .messages_changed(&before, &self.doc.frontiers(), &self.ids);
            self.ids = changes.ids_after.clone();
            changes
        }

        /// Write a raw field on a message, as a client that skips the core writers would.
        fn set(&mut self, id: &str, field: &str, value: serde_json::Value) -> MessageChanges {
            self.push(|d| d.set_message_fields(id, &[(field, value)]))
        }
    }

    fn poll() -> Poll {
        Poll::new(
            "Where?",
            &["Lisbon".into(), "Berlin".into()],
            false,
            false,
            None,
        )
        .unwrap()
    }

    fn as_user(user: &str) -> UserMessageOpts {
        UserMessageOpts {
            sender: Some(format!("user:{user}")),
            ..Default::default()
        }
    }

    fn code(c: &PushCheck) -> Option<&'static str> {
        match c {
            PushCheck::Refuse { code, .. } => Some(code),
            PushCheck::Allow { .. } => None,
        }
    }

    /// Alice and Bob in a chat, with Alice's poll `p1` posted and attested.
    fn chat_with_a_poll(state: &AppState) -> (Room, String, String, String, Principal, Principal) {
        let (chat, alice, bob, alice_dev, bob_dev) = chat_with_two(state);
        let mut room = Room::new();
        let c = room.push(|d| {
            d.append_poll_message("p1", &poll(), "dev", &as_user(&alice))
                .unwrap()
        });
        let PushCheck::Allow {
            carries_new_message,
            attributed,
        } = check_push(state, &alice_dev, PEOPLE_PROJECT, &chat, false, &c)
        else {
            panic!("a poll within bounds is a message like any other");
        };
        assert!(carries_new_message);
        assert_eq!(attributed, [("p1".to_owned(), format!("user:{alice}"))]);
        state
            .db
            .record_message_author(&chat, "p1", &format!("user:{alice}"))
            .unwrap();
        (room, chat, alice, bob, alice_dev, bob_dev)
    }

    #[test]
    fn a_member_casts_changes_and_retracts_their_own_vote() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, _alice, bob, _alice_dev, bob_dev) = chat_with_a_poll(&state);
        let me = format!("user:{bob}");
        for picked in [vec!["1".to_owned()], vec!["2".to_owned()], vec![]] {
            let c = room.push(|d| d.set_poll_vote("p1", &me, &picked).unwrap());
            assert_eq!(
                kind(&check_push(
                    &state,
                    &bob_dev,
                    PEOPLE_PROJECT,
                    &chat,
                    false,
                    &c
                )),
                Some(false),
                "a vote is not a new message: {picked:?}"
            );
        }
    }

    #[test]
    fn a_poll_out_of_bounds_is_refused() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, _bob, alice_dev, _bob_dev) = chat_with_two(&state);
        let good = serde_json::to_value(poll()).unwrap();
        type Edit = fn(&mut serde_json::Value);
        let cases: [(&str, Edit); 8] = [
            ("a visibility flag that is not a boolean", |p| {
                p["showVoters"] = json!("yes")
            }),
            ("a closing time that is not a time", |p| {
                p["closesAt"] = json!("tomorrow")
            }),
            ("a closing time out of range", |p| {
                p["closesAt"] = json!(arbos_core::poll::TIME_MAX + 1)
            }),
            ("an unknown field", |p| p["anonymous"] = json!(true)),
            ("one option", |p| {
                p["options"].as_array_mut().unwrap().truncate(1)
            }),
            ("eleven options", |p| {
                p["options"] = (0..11)
                    .map(|i| json!({ "id": i.to_string(), "label": "x" }))
                    .collect()
            }),
            ("a question over the limit", |p| {
                p["question"] = json!("q".repeat(301))
            }),
            ("a wrong type", |p| p["max"] = json!("1")),
        ];
        for (what, edit) in cases {
            let mut bad = good.clone();
            edit(&mut bad);
            let mut room = Room::new();
            let c = room.push(|d| {
                d.append_user_message("p_bad", "Poll: x", "dev", &as_user(&alice))
                    .unwrap();
                d.set_message_fields("p_bad", &[(POLL_FIELD, bad)]);
            });
            assert_eq!(
                code(&check_push(
                    &state,
                    &alice_dev,
                    PEOPLE_PROJECT,
                    &chat,
                    false,
                    &c
                )),
                Some("bad_poll"),
                "{what}"
            );
        }
    }

    #[test]
    fn a_poll_cannot_be_edited_or_hung_on_an_existing_message() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, alice, _bob, alice_dev, _bob_dev) = chat_with_a_poll(&state);
        let mut other = poll();
        other.question = "Who pays?".into();

        // Not even by its author: people answered the question as it was asked.
        let c = room.set("p1", POLL_FIELD, serde_json::to_value(&other).unwrap());
        assert_eq!(
            code(&check_push(
                &state,
                &alice_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some("poll_fixed")
        );

        room.push(|d| {
            d.append_user_message("plain", "hello", "dev", &as_user(&alice))
                .unwrap()
        });
        let c = room.set("plain", POLL_FIELD, serde_json::to_value(&other).unwrap());
        assert_eq!(
            code(&check_push(
                &state,
                &alice_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some("poll_fixed")
        );
    }

    /// The voter is the socket, never the name in the key.
    #[test]
    fn a_vote_cannot_be_cast_changed_or_removed_under_someone_elses_name() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, alice, _bob, alice_dev, bob_dev) = chat_with_a_poll(&state);
        let alices = format!("{POLL_VOTE_PREFIX}user:{alice}");
        // Her vote's time is already on the row, so what is judged below is the vote.
        room.set(
            "p1",
            &format!("{POLL_VOTE_AT_PREFIX}user:{alice}"),
            json!(5),
        );

        let c = room.set("p1", &alices, json!(["2"]));
        assert_eq!(
            code(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some("not_yours"),
            "Bob writing Alice's vote"
        );
        // The same bytes from Alice's own socket are her vote.
        assert_eq!(
            code(&check_push(
                &state,
                &alice_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            None
        );
        // Emptying it is still writing it.
        let c = room.set("p1", &alices, json!([]));
        assert_eq!(
            code(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                true,
                &c
            )),
            Some("not_yours"),
            "claiming a heal changes nothing"
        );
        // Removing the field outright is refused for anyone: a retraction is an empty list.
        let c = room.set("p1", &alices, serde_json::Value::Null);
        let c2 = room.push(|d| {
            d.find_message("p1").unwrap().delete(&alices).unwrap();
        });
        for (c, who) in [(&c, &bob_dev), (&c2, &bob_dev), (&c2, &alice_dev)] {
            assert_eq!(
                code(&check_push(&state, who, PEOPLE_PROJECT, &chat, false, c)),
                Some("bad_vote")
            );
        }
    }

    #[test]
    fn a_vote_the_poll_does_not_allow_is_refused() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, alice, bob, _alice_dev, bob_dev) = chat_with_a_poll(&state);
        let mine = format!("{POLL_VOTE_PREFIX}user:{bob}");
        for (what, value) in [
            ("an unknown option", json!(["9"])),
            ("two answers to a single-choice poll", json!(["1", "2"])),
            ("a repeat", json!(["1", "1"])),
            ("not a list", json!("1")),
            ("not strings", json!([1])),
            (
                "an object with extra fields",
                json!({ "selections": ["1"] }),
            ),
        ] {
            let c = room.set("p1", &mine, value);
            assert_eq!(
                code(&check_push(
                    &state,
                    &bob_dev,
                    PEOPLE_PROJECT,
                    &chat,
                    false,
                    &c
                )),
                Some("bad_vote"),
                "{what}"
            );
        }

        // A vote on a message that holds no poll is a vote on nothing.
        room.push(|d| {
            d.append_user_message("plain", "hello", "dev", &as_user(&alice))
                .unwrap()
        });
        let c = room.set("plain", &mine, json!(["1"]));
        assert_eq!(
            code(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some("bad_poll")
        );
    }

    /// A vote on a timed poll carries its time, and the time is the voter's field like the
    /// vote is: nobody moves someone else's vote to the far side of a close.
    #[test]
    fn a_votes_time_is_required_and_is_the_voters_to_write() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, alice, bob, alice_dev, bob_dev) = chat_with_a_poll(&state);
        let check = |c: &MessageChanges, who: &Principal| {
            code(&check_push(&state, who, PEOPLE_PROJECT, &chat, false, c))
        };
        let c = room.set("p1", &format!("{POLL_VOTE_PREFIX}user:{bob}"), json!(["1"]));
        assert_eq!(check(&c, &bob_dev), Some("bad_vote"), "a vote with no time");

        let c = room.push(|d| {
            d.set_poll_vote("p1", &format!("user:{alice}"), &["1".to_owned()])
                .unwrap()
        });
        assert_eq!(check(&c, &alice_dev), None);
        let alices = format!("{POLL_VOTE_AT_PREFIX}user:{alice}");
        let c = room.set("p1", &alices, json!(1));
        assert_eq!(
            check(&c, &bob_dev),
            Some("not_yours"),
            "Bob backdating Alice"
        );
        for bad in [json!("yesterday"), json!(0), json!(-1), json!(1.5)] {
            let c = room.set("p1", &alices, bad.clone());
            assert_eq!(check(&c, &alice_dev), Some("bad_vote"), "{bad}");
        }
    }

    /// Closing is the author's alone, by the relay's record of who posted the poll; the
    /// close is a time and stays on. A vote pushed after it is not refused — the reducer
    /// leaves it out — because an honest device may be delivering a vote cast before.
    #[test]
    fn only_the_author_closes_a_poll() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, _alice, _bob, alice_dev, bob_dev) = chat_with_a_poll(&state);
        let check = |c: &MessageChanges, who: &Principal, heal: bool| {
            code(&check_push(&state, who, PEOPLE_PROJECT, &chat, heal, c))
        };
        let c = room.set("p1", POLL_CLOSED_FIELD, json!(1_800_000_000_000i64));
        assert_eq!(check(&c, &bob_dev, false), Some("not_yours"));
        assert_eq!(check(&c, &bob_dev, true), Some("not_yours"), "a heal claim");
        assert_eq!(check(&c, &alice_dev, false), None);

        for bad in [json!("now"), json!(true), json!(0), json!(null)] {
            let c = room.set("p1", POLL_CLOSED_FIELD, bad.clone());
            assert_eq!(check(&c, &alice_dev, false), Some("bad_poll"), "{bad}");
        }
        let c = room.push(|d| {
            d.find_message("p1")
                .unwrap()
                .delete(POLL_CLOSED_FIELD)
                .unwrap();
        });
        assert_eq!(check(&c, &alice_dev, false), Some("bad_poll"), "reopening");
    }

    /// "Show who voted" is part of the poll field, which is written once: nobody, its author
    /// included, turns it on after people voted.
    #[test]
    fn the_visibility_flag_cannot_be_changed_after_posting() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, _alice, _bob, alice_dev, bob_dev) = chat_with_a_poll(&state);
        let mut shown = poll();
        assert_eq!(shown.show_voters, Some(false));
        shown.show_voters = Some(true);
        let c = room.set("p1", POLL_FIELD, serde_json::to_value(&shown).unwrap());
        for who in [&alice_dev, &bob_dev] {
            assert_eq!(
                code(&check_push(&state, who, PEOPLE_PROJECT, &chat, false, &c)),
                Some("poll_fixed")
            );
        }
    }

    /// A poll is deleted by its author or a moderator like any message, and by nobody else.
    #[test]
    fn a_poll_is_its_authors_to_delete() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, _alice, _bob, alice_dev, bob_dev) = chat_with_a_poll(&state);
        let c = room.push(|d| d.delete_message("p1").unwrap());
        assert_eq!(c.removed.len(), 1, "the poll row left the list");
        let check =
            |who: &Principal| code(&check_push(&state, who, PEOPLE_PROJECT, &chat, false, &c));
        assert_eq!(
            check(&bob_dev),
            Some("not_yours"),
            "another member of the DM"
        );
        assert_eq!(check(&alice_dev), None, "its author");
    }

    const NOW: i64 = 1_800_000_000_000;
    const MINUTE: i64 = 60_000;

    /// The poll's fields after the relay wrote `fixes` back, as every reader then reduces it.
    fn reduced(room: &Room, fixes: &[PollFix]) -> serde_json::Value {
        for fix in fixes {
            let fields: Vec<(&str, serde_json::Value)> = fix
                .fields
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect();
            room.doc.set_message_fields(&fix.message_id, &fields);
        }
        room.doc.projected_rows()[0]["parts"][1].clone()
    }

    /// Bob voted for Lisbon at `NOW - 10 min`; Alice closed the poll at `NOW - 5 min`.
    fn closed_poll(state: &AppState) -> (Room, String, String, String) {
        let (mut room, chat, alice, bob, _alice_dev, _bob_dev) = chat_with_a_poll(state);
        room.push(|d| {
            d.set_message_fields(
                "p1",
                &[
                    (&format!("{POLL_VOTE_PREFIX}user:{bob}"), json!(["1"])),
                    (
                        &format!("{POLL_VOTE_AT_PREFIX}user:{bob}"),
                        json!(NOW - 10 * MINUTE),
                    ),
                    (POLL_CLOSED_FIELD, json!(NOW - 5 * MINUTE)),
                ],
            )
        });
        (room, chat, alice, bob)
    }

    /// A close is final: the author's own later write of `pollClosed` — forward to reopen
    /// the poll, back to void votes already cast — is put back by the relay.
    #[test]
    fn a_close_cannot_be_rewritten() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, ..) = closed_poll(&state);
        let tally = json!({ "1": 1 });
        for (what, rewrite) in [
            ("reopened", NOW + 365 * 24 * 60 * MINUTE),
            ("backdated over Bob's vote", NOW - 11 * MINUTE),
        ] {
            let c = room.set("p1", POLL_CLOSED_FIELD, json!(rewrite));
            assert_ne!(
                reduced(&room, &[])["deadline"],
                NOW - 5 * MINUTE,
                "{what}: the push itself moved the close"
            );
            let fixes = poll_corrections(&state, &chat, &c, NOW);
            assert_eq!(
                fixes,
                [PollFix {
                    message_id: "p1".into(),
                    fields: vec![(POLL_CLOSED_FIELD.into(), json!(NOW - 5 * MINUTE))],
                }],
                "{what}"
            );
            let part = reduced(&room, &fixes);
            assert_eq!(part["deadline"], NOW - 5 * MINUTE, "{what}");
            assert_eq!(part["counts"], tally, "{what}: Bob's vote still counts");
        }
    }

    /// A first close takes the relay's time unless its own is at most the tolerance old: it
    /// cannot be dated back over votes, nor forward to stay open.
    #[test]
    fn a_first_close_is_timed_by_the_relay() {
        let state = test_state(AuthMode::Github);
        for (written, kept) in [
            (NOW - MINUTE, true),
            (NOW, true),
            (NOW - POLL_CLOCK_TOLERANCE_MS - 1, false),
            (NOW + 1, false),
            (1, false),
        ] {
            let (mut room, chat, ..) = chat_with_a_poll(&state);
            let c = room.set("p1", POLL_CLOSED_FIELD, json!(written));
            let fixes = poll_corrections(&state, &chat, &c, NOW);
            assert_eq!(fixes.is_empty(), kept, "{written}");
            let want = if kept { written } else { NOW };
            assert_eq!(reduced(&room, &fixes)["deadline"], want, "{written}");
        }
    }

    /// After the deadline by the relay's clock nothing about a vote changes: not the choice
    /// alone (riding on the old time), not a retraction, not a backdated first vote.
    #[test]
    fn a_vote_written_after_the_close_is_put_back() {
        let state = test_state(AuthMode::Github);
        let (.., alice, bob) = closed_poll(&state);
        let (vote, at) = (
            format!("{POLL_VOTE_PREFIX}user:{bob}"),
            format!("{POLL_VOTE_AT_PREFIX}user:{bob}"),
        );
        let cases: [(&str, Vec<(String, serde_json::Value)>); 4] = [
            (
                "the choice changed, the time untouched",
                vec![(vote.clone(), json!(["2"]))],
            ),
            ("a retraction", vec![(vote.clone(), json!([]))]),
            (
                "a changed choice with a backdated time",
                vec![
                    (vote.clone(), json!(["2"])),
                    (at.clone(), json!(NOW - 20 * MINUTE)),
                ],
            ),
            (
                "someone's backdated first vote",
                vec![
                    (format!("{POLL_VOTE_PREFIX}user:{alice}"), json!(["2"])),
                    (
                        format!("{POLL_VOTE_AT_PREFIX}user:{alice}"),
                        json!(NOW - 20 * MINUTE),
                    ),
                ],
            ),
        ];
        for (what, writes) in cases {
            let (mut room, chat, ..) = closed_poll(&state);
            let c = room.push(|d| {
                let fields: Vec<(&str, serde_json::Value)> = writes
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect();
                d.set_message_fields("p1", &fields)
            });
            assert_ne!(
                reduced(&room, &[])["counts"],
                json!({ "1": 1 }),
                "{what}: the push alone changes the count"
            );
            let fixes = poll_corrections(&state, &chat, &c, NOW);
            assert_eq!(
                reduced(&room, &fixes)["counts"],
                json!({ "1": 1 }),
                "{what}: the count the poll closed with"
            );
        }
    }

    /// While the poll is open a vote's time is the relay's: a time further than the
    /// tolerance from the relay's clock, or a vote changed without a new time, is stamped
    /// now. A time the device got about right is kept, and nothing more is written.
    #[test]
    fn an_open_polls_vote_is_timed_by_the_relay() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, _alice, bob, ..) = chat_with_a_poll(&state);
        let (vote, at) = (
            format!("{POLL_VOTE_PREFIX}user:{bob}"),
            format!("{POLL_VOTE_AT_PREFIX}user:{bob}"),
        );
        let mut push = |fields: &[(&str, serde_json::Value)]| {
            let c = room.push(|d| d.set_message_fields("p1", fields));
            poll_corrections(&state, &chat, &c, NOW)
        };
        let stamped = [PollFix {
            message_id: "p1".into(),
            fields: vec![(at.clone(), json!(NOW))],
        }];
        assert!(push(&[(&vote, json!(["1"])), (&at, json!(NOW - 1000))]).is_empty());
        assert!(push(&[(&vote, json!(["2"])), (&at, json!(NOW + 1000))]).is_empty());
        assert_eq!(
            push(&[(&vote, json!(["1"])), (&at, json!(NOW - 3 * MINUTE))]),
            stamped
        );
        assert_eq!(
            push(&[(&vote, json!(["2"])), (&at, json!(NOW + 3 * MINUTE))]),
            stamped
        );
        assert_eq!(
            push(&[(&at, json!(5))]),
            stamped,
            "the time alone, dated back"
        );
        assert_eq!(
            push(&[(&vote, json!(["1"]))]),
            stamped,
            "a vote with no new time"
        );
    }

    /// A vote cast before its device heard of the close, arriving after it: stored (a refusal
    /// would cut the device off the chat) and then emptied by the relay, so it is not counted.
    #[test]
    fn a_vote_arriving_after_the_close_is_stored_and_not_counted() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, alice, bob, _alice_dev, bob_dev) = chat_with_a_poll(&state);
        let mut bobs = ChatDoc::new(Some(2), false);
        bobs.import(&room.doc.snapshot()).unwrap();
        room.push(|d| d.close_poll("p1", &format!("user:{alice}")).unwrap());
        bobs.transact(|d| {
            d.set_poll_vote("p1", &format!("user:{bob}"), &["1".to_owned()])
                .unwrap()
        });
        let update = bobs.snapshot();
        let before = room.doc.frontiers();
        room.doc.import(&update).unwrap();
        let c = room
            .doc
            .messages_changed(&before, &room.doc.frontiers(), &room.ids);
        assert_eq!(
            code(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            None,
            "an honest device's push is never refused"
        );
        let fixes = poll_corrections(&state, &chat, &c, arbos_core::ids::now_ms() + 1);
        assert_eq!(reduced(&room, &fixes)["voters"], 0);
    }

    /// A row the relay attested before, arriving whole, is history a heal restores: its
    /// votes, their times and its close are left as they come.
    #[test]
    fn a_healed_poll_keeps_its_count() {
        let state = test_state(AuthMode::Github);
        let (source, chat, ..) = closed_poll(&state);
        let mut lost = Room::new();
        let snapshot = source.doc.snapshot();
        let before = lost.doc.frontiers();
        lost.doc.import(&snapshot).unwrap();
        let c = lost
            .doc
            .messages_changed(&before, &lost.doc.frontiers(), &lost.ids);
        assert_eq!(c.inserted.len(), 1, "the poll row arrives whole");
        assert!(poll_corrections(&state, &chat, &c, NOW).is_empty());
        assert_eq!(reduced(&lost, &[])["counts"], json!({ "1": 1 }));
    }

    #[test]
    fn only_a_member_of_the_chat_may_vote() {
        let state = test_state(AuthMode::Github);
        let (mut room, chat, _alice, _bob, _alice_dev, _bob_dev) = chat_with_a_poll(&state);
        let (carol, carol_dev) = person(&state, 103);
        let c = room.push(|d| {
            d.set_poll_vote("p1", &format!("user:{carol}"), &["1".to_owned()])
                .unwrap()
        });
        assert_eq!(
            code(&check_push(
                &state,
                &carol_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some("not_a_member")
        );
    }

    /// A poll cannot arrive with other people's votes already on it — unless the relay has
    /// attested that row before, which is a heal restoring it with the votes it had.
    #[test]
    fn a_new_poll_cannot_arrive_with_someone_elses_vote_on_it() {
        let state = test_state(AuthMode::Github);
        let (chat, alice, bob, alice_dev, bob_dev) = chat_with_two(&state);
        let stuffed = |id: &str| {
            let mut room = Room::new();
            room.push(|d| {
                d.append_poll_message(id, &poll(), "dev", &as_user(&alice))
                    .unwrap();
                d.set_poll_vote(id, &format!("user:{alice}"), &["1".to_owned()])
                    .unwrap();
                d.set_poll_vote(id, &format!("user:{bob}"), &["1".to_owned()])
                    .unwrap();
            })
        };
        let c = stuffed("p_new");
        assert_eq!(
            code(&check_push(
                &state,
                &alice_dev,
                PEOPLE_PROJECT,
                &chat,
                false,
                &c
            )),
            Some("not_yours"),
            "the author voting for Bob on her own poll"
        );
        assert_eq!(
            code(&check_push(
                &state,
                &alice_dev,
                PEOPLE_PROJECT,
                &chat,
                true,
                &c
            )),
            Some("not_yours"),
            "the heal flag is a claim; the record is what decides"
        );

        // `m_alice` is a row the relay attested. Bob's device restoring it restores its votes.
        let c = stuffed("m_alice");
        assert_eq!(
            kind(&check_push(
                &state,
                &bob_dev,
                PEOPLE_PROJECT,
                &chat,
                true,
                &c
            )),
            Some(false)
        );
    }
}
