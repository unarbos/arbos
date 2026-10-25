//! The one presence row over the composer ([feedback
//! typing-indicator-left-2026-10-23], Jacob 2026-10-23): who is typing (people)
//! or working (agents) in the open chat, as a stack of round profile pictures
//! and ONE line of text — "Sam is typing…", "Lium and Home are working" — that
//! starts flush with the chat column's left edge, like the bubbles and the
//! pill. It replaces two older pieces that could both show at once: the
//! transcript's "<name> is working" indicator row (which, in a people chat, sat
//! under a sender caption with no bubble yet → a doubled notice) and the
//! "@sam is typing…" line over the composer.

use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, App, Div, RenderImage};

use crate::app::AppState;
use crate::model::{Message, MessageStatus, Part, Person, Project, Role, StatusKind};
use crate::ui::avatars;
use crate::ui::identity;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::theme::{Theme, TEXT_SMALL};

/// Round picture size: the avatar column's disc (`transcript::SENDER_DISC` — the
/// composer's `+` disc, which the sender discs beside the bubbles also use), so
/// the row's picture sits under the bubble discs and over the `+` pixel for
/// pixel (Jacob, 2026-10-23 15:15 / 15:20).
pub const AVATAR: f32 = crate::ui::transcript::SENDER_DISC;
/// Where the first picture starts inside the chat column (= the `+` disc's x).
pub const INSET: f32 = crate::ui::transcript::AVATAR_COLUMN_INSET;
/// How far each further picture tucks under the one before it (Grok / Telegram stack).
pub const OVERLAP: f32 = 8.0;
/// Pictures → text: the disc → bubble gap, so the text starts where the bubble body starts.
pub const GAP: f32 = crate::ui::transcript::SENDER_DISC_GAP;
/// Pictures shown before "+N".
pub const MAX_PICTURES: usize = 3;
/// Row height: the picture plus 3 pt of air above and below.
pub const ROW_HEIGHT: f32 = AVATAR + 6.0;
/// Air between the row and the composer pill.
pub const GAP_BELOW: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Typing,
    Working,
}

/// One person or agent on the row, newest first in [`Presence::participants`].
pub struct Participant {
    pub name: String,
    pub activity: Activity,
    pub picture: AnyElement,
}

pub struct Presence {
    pub participants: Vec<Participant>,
    /// The harness's own words ("Cloning RaoFoundation/subtensor") while exactly
    /// one agent works; `None` → "<name> is working".
    pub detail: Option<String>,
    /// The message waits for a sleeping machine (amber copy).
    pub queued: bool,
}

impl Presence {
    /// One line. Alone, a participant keeps the full display name; from two on,
    /// first names ("Mira and Sam are typing…"), so the line stays one line.
    pub fn text(&self) -> String {
        let several = self.participants.len() > 1;
        let name = |p: &Participant| -> String {
            if several {
                first_name(&p.name)
            } else {
                p.name.clone()
            }
        };
        let typing: Vec<String> = self
            .participants
            .iter()
            .filter(|p| p.activity == Activity::Typing)
            .map(name)
            .collect();
        let working: Vec<String> = self
            .participants
            .iter()
            .filter(|p| p.activity == Activity::Working)
            .map(name)
            .collect();
        let typing: Vec<&str> = typing.iter().map(String::as_str).collect();
        let working: Vec<&str> = working.iter().map(String::as_str).collect();
        sentence(&typing, &working, self.detail.as_deref(), self.queued)
    }
}

/// "Sam Okafor" → "Sam"; "@sam" and one-word names stay as they are.
pub fn first_name(name: &str) -> String {
    name.split_whitespace().next().unwrap_or(name).to_string()
}

/// "A", "A and B", "A, B and C", "A, B and 2 others".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [a] => a.to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} others", rest.len()),
    }
}

fn verb(n: usize) -> &'static str {
    if n == 1 {
        "is"
    } else {
        "are"
    }
}

/// The row's one line. Typing and working groups each get a clause; both →
/// joined with " · ". Exactly one worker with a `detail` shows the detail;
/// a queued single worker "is waiting for the machine".
pub fn sentence(typing: &[&str], working: &[&str], detail: Option<&str>, queued: bool) -> String {
    let mut clauses: Vec<String> = Vec::new();
    if !typing.is_empty() {
        clauses.push(format!(
            "{} {} typing…",
            join_names(typing),
            verb(typing.len())
        ));
    }
    if !working.is_empty() {
        let clause = match (working, queued, detail) {
            ([one], true, _) => format!("{one} is waiting for the machine"),
            ([_], false, Some(d)) if !d.trim().is_empty() => d.trim().to_string(),
            _ => format!("{} {} working", join_names(working), verb(working.len())),
        };
        clauses.push(clause);
    }
    clauses.join(" · ")
}

/// Everyone typing or working in `chat_id`, newest first; `None` when the row
/// has nothing to say. Pictures are the same ones the header pill and the
/// rows use for that person / agent (D79 disc, D92 leaf, or the photo).
pub fn gather(state: &AppState, chat_id: &str, space: &Project, theme: &Theme) -> Option<Presence> {
    let now = crate::model::now_ms();
    let mut participants: Vec<Participant> = Vec::new();

    // People typing (room presence, people chats only).
    for handle in state.typing_in(chat_id) {
        let person = space.people.iter().find(|p| p.handle == handle).cloned();
        let (name, picture) = match person {
            Some(p) => {
                let image: Option<Arc<RenderImage>> = state.avatar(&p.avatar_url);
                (
                    p.display_name(),
                    people::avatar(image, &p, AVATAR, theme).into_any_element(),
                )
            }
            None => {
                let p = Person {
                    handle: handle.clone(),
                    ..Default::default()
                };
                (
                    format!("@{handle}"),
                    avatars::person(&p, AVATAR, theme).into_any_element(),
                )
            }
        };
        participants.push(Participant {
            name,
            activity: Activity::Typing,
            picture,
        });
    }

    // Agents working: a fresh stream or an open hand-off, per sender.
    let messages = state.messages_of(chat_id);
    let running = Message::chat_is_actively_working(messages, now);
    let queued = messages.iter().any(|m| {
        (m.role == Role::User && m.status == MessageStatus::Queued)
            || (m.is_streaming()
                && m.parts.iter().any(|p| {
                    matches!(
                        p,
                        Part::Status {
                            code: StatusKind::Queued,
                            ..
                        }
                    )
                }))
    });
    let mut detail: Option<String> = None;
    if running || queued {
        let recent = now.saturating_sub(Message::STREAM_FRESH_MS);
        let live = |m: &Message| {
            m.role == Role::Assistant
                && (m.is_streaming() && m.created_at >= recent
                    || Message::chat_has_open_subagent(std::slice::from_ref(m), now))
        };
        // The newest live row that has words: a short main turn streaming
        // with no activity yet must not blank the job's "Reading … · 2 tasks"
        // under it (duplex §2.4: the line is the newest step of the newest job).
        detail = messages
            .iter()
            .rev()
            .filter(|m| m.role == Role::Assistant && m.is_streaming() && m.created_at >= recent)
            .find_map(Message::current_activity);
        if space.is_people() {
            let mut seen: Vec<String> = Vec::new();
            for m in messages.iter().rev().filter(|m| live(m)) {
                // The same question the bubble asks (#1592): the row's sender, handle and
                // name against the roster, my projects, the other rosters — one identity,
                // so the row's picture is the bubble's disc and its words the caption.
                let agent = avatars::agent_identity(
                    Some(space),
                    &state.projects,
                    &m.sender,
                    m.sender_handle.as_deref(),
                    m.sender_name.as_deref(),
                );
                if seen.contains(&agent.id) {
                    continue;
                }
                seen.push(agent.id.clone());
                let image = agent.image_key.as_deref().and_then(|url| state.avatar(url));
                participants.push(Participant {
                    name: agent.name.clone(),
                    activity: Activity::Working,
                    picture: avatars::agent_picture(&agent, image, AVATAR, theme)
                        .into_any_element(),
                });
            }
        } else {
            let agent =
                avatars::agent_identity(None, &state.projects, &space.id, None, Some(&space.name));
            let image = agent.image_key.as_deref().and_then(|url| state.avatar(url));
            participants.push(Participant {
                name: agent.name.clone(),
                activity: Activity::Working,
                picture: avatars::agent_picture(&agent, image, AVATAR, theme).into_any_element(),
            });
        }
    }

    if participants.is_empty() {
        return None;
    }
    Some(Presence {
        participants,
        detail,
        queued,
    })
}

/// The row itself: the picture stack (identity slots, newest in front, cap +
/// "+N"), then one muted line that breathes with the pulse clock. Left-bound
/// inside whatever frames it — the shell puts it through
/// `transcript::column_row`, so its left edge is the composer's.
pub fn row(presence: Presence, theme: &Theme, cx: &App) -> Div {
    row_inner(presence, None, theme, cx)
}

/// The row with the tasks chip after its line (`ui::tasks`, duplex §2.4):
/// the box's "· N tasks" suffix leaves the words — the chip says the count —
/// and the chip sits one row gap after the text, on the line's centre.
pub fn row_with_chip(presence: Presence, chip: AnyElement, theme: &Theme, cx: &App) -> Div {
    row_inner(presence, Some(chip), theme, cx)
}

fn row_inner(presence: Presence, chip: Option<AnyElement>, theme: &Theme, cx: &App) -> Div {
    let phase = PulseClock::phase(cx, 1100);
    let text = presence.text();
    let text = if chip.is_some() {
        crate::model::Job::strip_count_suffix(&text).to_string()
    } else {
        text
    };
    let total = presence.participants.len();
    // Taffy reports a zero content width for a row whose children carry
    // negative margins, so the stack states its width: one picture plus
    // (AVATAR − OVERLAP) per further disc (the "+N" disc counts as one).
    let discs = total.min(MAX_PICTURES) + usize::from(total > MAX_PICTURES);
    let stack_w = AVATAR + (discs.saturating_sub(1) as f32) * (AVATAR - OVERLAP);
    let mut stack = div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .w(px(stack_w))
        .h(px(AVATAR))
        .debug_selector(|| "presence-pictures".into());
    for (i, p) in presence
        .participants
        .into_iter()
        .take(MAX_PICTURES)
        .enumerate()
    {
        // The slot is the bubble disc's exact box (no border inside it — a rim
        // would shave a point off the picture). With several pictures a ring
        // in the page colour sits just *outside* each one so the overlaps read
        // (newest first on the left, each next one tucked 8 pt in).
        let slot = identity::slot(AVATAR, p.picture)
            .relative()
            .rounded_full()
            .when(total > 1, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset(px(-1.0))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.bg),
                )
            })
            .debug_selector(move || format!("presence-picture-{i}"));
        stack = stack.child(if i == 0 { slot } else { slot.ml(px(-OVERLAP)) });
    }
    if total > MAX_PICTURES {
        stack = stack.child(
            div()
                .size(px(AVATAR))
                .ml(px(-OVERLAP))
                .flex_shrink_0()
                .rounded_full()
                .bg(theme.field)
                .border_1()
                .border_color(theme.bg)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.0))
                .text_color(theme.text_muted)
                .debug_selector(|| "presence-more".into())
                .child(format!("+{}", total - MAX_PICTURES)),
        );
    }
    let color = if presence.queued {
        theme.warn
    } else {
        theme.text_muted
    };
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .mb(px(GAP_BELOW))
        .pl(px(INSET))
        .flex()
        .items_center()
        .justify_start()
        .gap(px(GAP))
        .debug_selector(|| "presence-row".into())
        .child(stack)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_SMALL))
                .text_color(color)
                .opacity(0.55 + 0.45 * phase)
                .debug_selector(|| "presence-text".into())
                .child(text),
        )
        // The row's own gap (14, the disc → bubble gap) separates text and chip.
        .children(chip.map(|chip| div().flex_shrink_0().child(chip)))
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn one_line_whatever_the_count() {
        assert_eq!(sentence(&["Sam"], &[], None, false), "Sam is typing…");
        assert_eq!(
            sentence(&["Sam", "Samir"], &[], None, false),
            "Sam and Samir are typing…"
        );
        assert_eq!(
            sentence(&["Sam", "Samir", "Kai", "Ava"], &[], None, false),
            "Sam, Samir and 2 others are typing…"
        );
        assert_eq!(sentence(&[], &["Lium"], None, false), "Lium is working");
        assert_eq!(
            sentence(&[], &["Lium", "Home"], None, false),
            "Lium and Home are working"
        );
        assert_eq!(
            sentence(&[], &["Lium", "Home", "Doug"], None, false),
            "Lium, Home and Doug are working"
        );
    }

    #[test]
    fn detail_and_queue_apply_to_a_single_worker_only() {
        assert_eq!(
            sentence(
                &[],
                &["Lium"],
                Some("Cloning RaoFoundation/subtensor"),
                false
            ),
            "Cloning RaoFoundation/subtensor"
        );
        assert_eq!(
            sentence(&[], &["Lium", "Home"], Some("Cloning"), false),
            "Lium and Home are working"
        );
        assert_eq!(
            sentence(&[], &["Lium"], None, true),
            "Lium is waiting for the machine"
        );
    }

    #[test]
    fn first_names_from_two_participants_on() {
        let p = |name: &str, activity| Participant {
            name: name.to_string(),
            activity,
            picture: gpui::Empty.into_any_element(),
        };
        let one = Presence {
            participants: vec![p("Sam Okafor", Activity::Typing)],
            detail: None,
            queued: false,
        };
        assert_eq!(one.text(), "Sam Okafor is typing…");
        let two = Presence {
            participants: vec![
                p("Mira Kaplan", Activity::Typing),
                p("Sam Okafor", Activity::Typing),
            ],
            detail: None,
            queued: false,
        };
        assert_eq!(two.text(), "Mira and Sam are typing…");
        assert_eq!(first_name("@sam"), "@sam");
    }

    #[test]
    fn typing_and_working_share_the_line() {
        assert_eq!(
            sentence(&["Sam"], &["Lium"], None, false),
            "Sam is typing… · Lium is working"
        );
        assert_eq!(sentence(&[], &[], None, false), "");
    }
}

#[cfg(test)]
mod identity_tests {
    //! #1592: the presence row and the bubble disc ask one resolver with the same
    //! row, so a foreign agent (someone else's TestAgent in Arbonites) gets the
    //! same picture and the same words on both — the roster's leaf and its display
    //! name, never a handle-seeded initials disc or the raw `@slug`.
    use super::*;
    use crate::model::{AgentMember, SpaceKind};
    use crate::ui::avatars::{leaf, LeafPick};

    fn roster_agent(id: &str, name: &str, image: Option<&str>) -> AgentMember {
        AgentMember {
            project_id: id.into(),
            name: name.into(),
            handle: Project::mention_slug(name),
            owner_handle: "sami".into(),
            avatar_leaf: Some(5),
            avatar_leaf_color: Some(2),
            image_url: image.map(str::to_owned),
            ..Default::default()
        }
    }

    fn arbonites(agents: Vec<AgentMember>) -> Project {
        Project {
            id: "chat_arbonites".into(),
            name: "Arbonites".into(),
            kind: SpaceKind::Group,
            agents,
            ..Default::default()
        }
    }

    fn working_row(sender: &str, handle: &str, name: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "m-1",
            "chatId": "chat_arbonites",
            "role": "assistant",
            "sender": sender,
            "senderHandle": handle,
            "senderName": name,
            "status": "streaming",
            "createdAt": crate::model::now_ms(),
            "parts": [{ "id": "s-1", "kind": "status", "code": "working", "message": "Working…" }],
        }))
        .expect("message")
    }

    /// What the bubble asks (`transcript::agent_identity_full`): the lane seed, no hint.
    fn bubble(space: &Project, mine: &[Project], m: &Message) -> avatars::AgentIdentity {
        avatars::agent_identity(Some(space), mine, &m.sender, None, None)
    }

    /// What the presence row asks (`gather`): the row's sender, handle and name.
    fn presence(space: &Project, mine: &[Project], m: &Message) -> avatars::AgentIdentity {
        avatars::agent_identity(
            Some(space),
            mine,
            &m.sender,
            m.sender_handle.as_deref(),
            m.sender_name.as_deref(),
        )
    }

    #[test]
    fn a_foreign_agents_row_and_bubble_share_one_identity() {
        let space = arbonites(vec![roster_agent("01TEST", "TestAgent", None)]);
        let mine = [Project {
            id: "01MINE".into(),
            name: "Mine".into(),
            ..Default::default()
        }];
        let m = working_row("agent:01TEST", "testagent", "TestAgent");
        let b = bubble(&space, &mine, &m);
        let p = presence(&space, &mine, &m);
        assert_eq!(
            b, p,
            "one identity for the bubble disc and the presence picture"
        );
        assert!(p.known, "the roster answered, not a fallback");
        assert_eq!(p.name, "TestAgent", "the display name, not the handle");
        assert_eq!(p.handle, "testagent");
        assert_eq!(
            p.leaf,
            LeafPick { leaf: 5, color: 2 },
            "the roster's leaf picks"
        );
        assert_eq!(p.id, "01TEST", "seeded by the bare id");
    }

    #[test]
    fn an_uploaded_picture_reaches_the_presence_row_too() {
        let space = arbonites(vec![roster_agent(
            "01TEST",
            "TestAgent",
            Some("https://relay/blob/01TEST/avatar-1.jpg?token=t"),
        )]);
        let m = working_row("agent:01TEST", "testagent", "TestAgent");
        let p = presence(&space, &[], &m);
        assert_eq!(
            p.image_key.as_deref(),
            Some("https://relay/blob/01TEST/avatar-1.jpg?token=t"),
            "the presence row gets the same picture URL the bubble disc loads"
        );
        assert_eq!(bubble(&space, &[], &m), p);
    }

    #[test]
    fn an_agent_missing_from_every_roster_still_agrees_with_itself() {
        let space = arbonites(Vec::new());
        let m = working_row("agent:01GHOST", "ghost", "Ghost Writer");
        let b = bubble(&space, &[], &m);
        let p = presence(&space, &[], &m);
        assert_eq!(b.id, p.id);
        assert_eq!(b.leaf, p.leaf, "both seed the leaf with the bare id");
        assert_eq!(p.leaf, leaf::default_pick("01GHOST"));
        assert_ne!(
            p.leaf,
            leaf::default_pick("agent:01GHOST"),
            "the old presence fallback seeded with the prefixed sender"
        );
        assert_eq!(p.name, "Ghost Writer", "the row's own name, never `ghost`");
        assert!(!p.known);
    }

    #[test]
    fn the_rosters_card_wins_over_a_stale_copy_in_my_projects() {
        let space = arbonites(vec![roster_agent("01HOME", "GigChadAgent", None)]);
        let mine = [Project {
            id: "01HOME".into(),
            name: "Home".into(),
            ..Default::default()
        }];
        let m = working_row("agent:01HOME", "gigchadagent", "GigChadAgent");
        assert_eq!(presence(&space, &mine, &m).name, "GigChadAgent");
        assert_eq!(bubble(&space, &mine, &m).name, "GigChadAgent");
    }
}
