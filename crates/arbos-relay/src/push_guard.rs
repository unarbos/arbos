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
//! Nothing here reads the pushed bytes for the *kind* of push, and that is deliberate. The
//! client authors its own CRDT ops, so anything derivable from them — op order, Lamport
//! clocks, which frontier they descend from — it can also shape at will. The relay's own
//! record of what it was told is the only part of the picture the pusher did not write.

use arbos_core::doc::MessageChanges;

use crate::auth::Principal;
use crate::moderation;
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

    for id in &changes.removed {
        let mine = author_of(id).is_some_and(|a| me.owns(state, &a));
        if !mine && !moderates() {
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

    /// Finding 1: the bypass. A push that deletes a message the relay attests to someone else
    /// is refused, which is what `moderation.rs` could not do from the HTTP route alone.
    #[test]
    fn a_push_cannot_delete_a_message_the_relay_attests_to_someone_else() {
        let state = test_state(AuthMode::Github);
        let (chat, _alice, _bob, alice_dev, bob_dev) = chat_with_two(&state);

        let mut c = changes();
        c.removed = vec!["m_alice".to_owned()];
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
        c.removed = vec!["m_bob".to_owned()];
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
        c.removed = vec!["m_bob".to_owned()];
        assert!(!refused(&check_push(
            &state,
            &alice_dev,
            PEOPLE_PROJECT,
            &chat,
            false,
            &c
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
}
