//! Settings → Agent access link (Jacob, approved mockups 2026-11-03): every
//! token this account ever handed an agent, in one list, plus the Name, Scope
//! and Expires controls that mint the next one.
//!
//! One copy, everywhere: the owner copies **one** URL — a single-use open
//! secret whose response body carries the agent's bearer — so no screen in
//! this flow ever shows a token (`docs/agent-access-link-spec.md` §5.1).
//!
//! The section also owns the read-only reader tokens that used to sit under
//! Settings → Devices (`kind = "reader"`): the relay returns them in the same
//! list with `origin: "export"`, and Devices lists sign-ins only.
//!
//! This module is the pure half (what the rows say and which choice is live);
//! `profiles/me.rs` draws it with the pane kit and `agent_link_reveal.rs`
//! draws the copy sheet.

pub use crate::agent_link_expiry::{EXPIRIES, EXPIRY_DEFAULT_HOURS};
use crate::model::AgentLinkView;

/// What a link may do. Two values, fixed at mint: to widen one, revoke it and
/// make another (`spec` §5.2). `Read` is always the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Read,
    Reply,
}

impl Scope {
    /// The relay's wire value (`POST /me/agent-links {scope}`).
    pub fn wire(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Reply => "reply",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Scope::Read => "Read only",
            Scope::Reply => "Read + reply as me",
        }
    }

    /// The consequence under the title, in the row itself.
    pub fn subtitle(self) -> &'static str {
        match self {
            Scope::Read => "It can read your chats. It cannot post.",
            Scope::Reply => "It can post as you. People see your name.",
        }
    }

    /// The card's footer, which swaps with the choice: the dangerous sentence
    /// is never on screen while the safe option is selected, so it cannot
    /// become furniture the eye skips.
    pub fn consequence(self) -> &'static str {
        match self {
            Scope::Read => {
                "This link can only read. Nothing it does appears as a message from you."
            }
            Scope::Reply => {
                "Replies from this link are from you. Nobody you talk to can tell them apart \
                 from your own messages."
            }
        }
    }

    /// The Create button names the scope it is about to grant, so the
    /// consequence is restated at the moment of commitment.
    pub fn create_label(self) -> &'static str {
        match self {
            Scope::Read => "Create read-only link",
            Scope::Reply => "Create reply-as-me link",
        }
    }

    /// Red ink: the label, the subtitle, the checkmark and the Create button
    /// all take `theme.danger` while reply-as-me is chosen. Apart from Revoke
    /// that is the only red in the section, so red means one thing here.
    pub fn is_danger(self) -> bool {
        self == Scope::Reply
    }

    /// The list's and the copy sheet's capsule after the name.
    pub fn tag(self) -> &'static str {
        match self {
            Scope::Read => "read only",
            Scope::Reply => "replies as you",
        }
    }

    /// A relay row's `scope`; anything unreadable is the weaker of the two.
    pub fn from_wire(raw: &str) -> Self {
        match raw.trim() {
            "reply" => Scope::Reply,
            _ => Scope::Read,
        }
    }
}

/// The scope rows, in order. Read only leads and is never not the default.
pub const SCOPES: [Scope; 2] = [Scope::Read, Scope::Reply];

/// The copy sheet's own once-ness lines, split in two so neither runs long.
/// The once-ness belongs to the link, because the link *is* the credential:
/// the single-use secret is in the URL and the bearer only ever exists in the
/// body of the one response it fetches.
pub const ONCE_NOTE: &str =
    "This link opens once. Give it to your agent now — you will not see this URL again.";
pub const RACE_NOTE: &str = "Whatever opens it first gets the access, and your agent then sees \
                             an error. If that happens, revoke this link and make a new one.";

/// The copy sheet restates what was just minted, so nobody copies a
/// reply-as-me link believing it is read only.
pub fn restatement(scope: Scope, expires_in: &str) -> String {
    let what = match scope {
        Scope::Read => "This agent can read your chats and cannot post.",
        Scope::Reply => "Anything this agent posts is from you.",
    };
    format!("{what} Expires in {expires_in}.")
}

/// What one row of the Active links list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRow {
    pub id: String,
    /// The owner's label for it ("Jacob's Claude"), never a link code: once a
    /// link is single-use there is no URL left to recognise it by.
    pub name: String,
    pub scope: Scope,
    /// "Created Tue · used 2 h ago" / "Created 25 Oct · never opened".
    pub subtitle: String,
}

/// The links that are still worth a row: not revoked, not past their expiry.
/// The relay returns the dead ones too (they are the audit's subjects); this
/// list is called Active links and means it.
pub fn live(items: &[AgentLinkView]) -> impl Iterator<Item = &AgentLinkView> {
    items.iter().filter(|l| l.live)
}

/// The rows, in the relay's order (newest first, then the reader tokens that
/// predate links).
pub fn rows(items: &[AgentLinkView], now_ms: i64) -> Vec<LinkRow> {
    live(items).map(|l| row(l, now_ms)).collect()
}

fn row(l: &AgentLinkView, now_ms: i64) -> LinkRow {
    LinkRow {
        id: l.id.clone(),
        name: if l.label.trim().is_empty() {
            "Agent link".to_string()
        } else {
            l.label.trim().to_string()
        },
        scope: Scope::from_wire(&l.scope),
        subtitle: format!(
            "Created {} · {}",
            created_label(l.created_at, now_ms),
            used_label(l.opened_at, l.last_seen_at, now_ms)
        ),
    }
}

/// The weekday inside the last week ("Tue"), the date before that ("25 Oct").
/// Coarse, because the created time is the fact you already know.
pub fn created_label(at: i64, now_ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    let Some(t) = Utc.timestamp_millis_opt(at).single() else {
        return "—".to_string();
    };
    if now_ms.saturating_sub(at) < 7 * 24 * 3_600_000 {
        t.format("%a").to_string()
    } else {
        t.format("%-d %b").to_string()
    }
}

/// "never opened" when nothing ever fetched the URL, else "used 4 min ago" —
/// fine-grained, because this is the fact you came to the page to check.
/// Jacob's case is handing a link to an outside debugger: whether the agent
/// ever picked it up is the whole answer.
pub fn used_label(opened_at: Option<i64>, last_seen_at: Option<i64>, now_ms: i64) -> String {
    if opened_at.is_none() {
        return "never opened".to_string();
    }
    match last_seen_at.or(opened_at) {
        Some(seen) => format!("used {}", ago(now_ms.saturating_sub(seen).max(0))),
        None => "never opened".to_string(),
    }
}

/// "just now" under two minutes, then minutes, hours, days.
fn ago(ms: i64) -> String {
    let min = ms / 60_000;
    if min < 2 {
        "just now".to_string()
    } else if min < 60 {
        format!("{min} min ago")
    } else if min < 24 * 60 {
        format!("{} h ago", min / 60)
    } else {
        format!("{} d ago", min / (24 * 60))
    }
}

/// `https://relay.arbos.life/agent-link/open/9kT2vQ…` →
/// `relay.arbos.life/…/9kT2vQ`. The row shows enough of the URL to tell two
/// copies apart and no more; Copy puts the whole thing on the clipboard.
pub fn short_url(url: &str) -> String {
    let bare = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let mut parts = bare.splitn(2, '/');
    let host = parts.next().unwrap_or("");
    let Some(path) = parts.next() else {
        return bare.to_owned();
    };
    let tail = path.rsplit('/').next().unwrap_or(path);
    let tail: String = if tail.chars().count() > 6 {
        tail.chars().skip(tail.chars().count() - 6).collect()
    } else {
        tail.to_owned()
    };
    format!("{host}/…/{tail}")
}

/// The longest label the relay accepts (`LABEL_MAX`); a longer name is cut to
/// it rather than sent and refused.
pub const LABEL_MAX: usize = 60;

/// The label to send for a link the owner named `typed`.
///
/// Never empty. The relay does not number an unlabelled link — it stores the
/// one literal "Agent link" — so the numbering is ours to compose and ours to
/// send, and a blank field becomes [`next_label`] here rather than an absent
/// `label` on the wire.
pub fn label_for(typed: &str, items: &[AgentLinkView]) -> String {
    let typed = typed.trim();
    if typed.is_empty() {
        return next_label(items);
    }
    typed.chars().take(LABEL_MAX).collect()
}

/// The label a link gets when the owner names it nothing. The relay defaults
/// every unlabelled link to the same "Agent link", and five of those in one
/// list tell the owner nothing — so the first is "Agent link" and each later
/// one counts up past whatever is already live.
pub fn next_label(items: &[AgentLinkView]) -> String {
    const BASE: &str = "Agent link";
    let taken: Vec<&str> = live(items).map(|l| l.label.trim()).collect();
    if !taken.contains(&BASE) {
        return BASE.to_owned();
    }
    (2..)
        .map(|n| format!("{BASE} {n}"))
        .find(|name| !taken.iter().any(|l| l == name))
        .unwrap_or_else(|| BASE.to_owned())
}

/// Drop a revoked link from the list at once, so the row goes with the click
/// and the relay's next list only confirms it.
pub fn remove(items: &mut Vec<AgentLinkView>, id: &str) -> bool {
    let before = items.len();
    items.retain(|l| l.id != id);
    items.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_762_200_000_000;

    fn link(id: &str, label: &str, scope: &str) -> AgentLinkView {
        AgentLinkView {
            id: id.into(),
            origin: "link".into(),
            label: label.into(),
            scope: scope.into(),
            created_at: NOW - 2 * 24 * 3_600_000,
            expires_at: Some(NOW + 5 * 24 * 3_600_000),
            opened_at: Some(NOW - 3 * 3_600_000),
            revoked_at: None,
            last_seen_at: Some(NOW - 2 * 3_600_000),
            live: true,
            state: "in_use".into(),
            note: None,
        }
    }

    /// Read only leads, is the default, and is the only choice with no red
    /// in it. Nothing on this screen pre-selects reply-as-me.
    #[test]
    fn read_only_leads_and_is_the_default() {
        assert_eq!(SCOPES, [Scope::Read, Scope::Reply]);
        assert_eq!(Scope::default(), Scope::Read);
        assert!(!Scope::Read.is_danger());
        assert!(Scope::Reply.is_danger());
        assert_eq!(Scope::Read.wire(), "read");
        assert_eq!(Scope::Reply.wire(), "reply");
    }

    /// The Create button and the card footer both restate the scope, so the
    /// consequence is on screen at the moment of commitment.
    #[test]
    fn the_create_button_and_the_footer_name_the_scope() {
        assert_eq!(Scope::Read.create_label(), "Create read-only link");
        assert_eq!(Scope::Reply.create_label(), "Create reply-as-me link");
        assert!(Scope::Reply.consequence().contains("are from you"));
        assert!(Scope::Read.consequence().contains("only read"));
        assert_ne!(Scope::Read.consequence(), Scope::Reply.consequence());
    }

    /// The spec's four expiries, 7 days in the middle as the default, and no
    /// "Never" anywhere in the set.
    #[test]
    fn the_expiry_set_is_the_specs_and_holds_no_never() {
        assert_eq!(
            EXPIRIES.map(|e| e.label),
            ["1 hour", "24 hours", "7 days", "30 days"]
        );
        assert_eq!(EXPIRIES.map(|e| e.hours), [1, 24, 168, 720]);
        assert_eq!(EXPIRY_DEFAULT_HOURS, 168);
        assert_eq!(
            crate::agent_link_expiry::expiry_label(EXPIRY_DEFAULT_HOURS),
            "7 days"
        );
        assert!(EXPIRIES.iter().all(|e| e.label != "Never"));
        // Every option is inside the relay's own 1 hour … 30 days window
        // (`agent_link::TTL_MIN_MS` / `TTL_MAX_MS`), so no choice on this
        // screen can be refused with `bad_ttl`.
        assert!(EXPIRIES.iter().all(|e| (1..=720).contains(&e.hours)));
    }

    #[test]
    fn a_scope_the_relay_sends_that_we_cannot_read_is_the_weaker_one() {
        assert_eq!(Scope::from_wire("reply"), Scope::Reply);
        assert_eq!(Scope::from_wire("read"), Scope::Read);
        assert_eq!(Scope::from_wire("admin"), Scope::Read);
        assert_eq!(Scope::from_wire(""), Scope::Read);
    }

    /// Revoked and expired links are in the relay's answer; "Active links"
    /// shows neither.
    #[test]
    fn only_live_links_reach_the_list() {
        let mut revoked = link("b", "Old debugger", "read");
        revoked.live = false;
        revoked.state = "revoked".into();
        revoked.revoked_at = Some(NOW - 3_600_000);
        let mut expired = link("c", "Last week's", "reply");
        expired.live = false;
        expired.state = "expired".into();
        let list = vec![link("a", "Jacob's Claude", "reply"), revoked, expired];
        assert_eq!(
            rows(&list, NOW)
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(live(&list).count(), 1);
    }

    /// The row reads as the mockup: the owner's label, the scope capsule, a
    /// coarse created date and a fine last-used time.
    #[test]
    fn the_rows_read_as_the_mockup() {
        let mut reader = link("r", "GrokBot-Arbos", "read");
        reader.origin = "export".into();
        reader.created_at = NOW - 9 * 24 * 3_600_000;
        reader.last_seen_at = Some(NOW - 3 * 24 * 3_600_000);
        let mut fresh = link("n", "Codex debugger", "read");
        fresh.opened_at = None;
        fresh.last_seen_at = None;
        let list = rows(&[link("a", "Jacob's Claude", "reply"), fresh, reader], NOW);
        assert_eq!(list[0].name, "Jacob's Claude");
        assert_eq!(list[0].scope, Scope::Reply);
        assert_eq!(list[0].scope.tag(), "replies as you");
        assert!(
            list[0].subtitle.ends_with(" · used 2 h ago"),
            "{}",
            list[0].subtitle
        );
        assert_eq!(list[1].subtitle.split(" · ").nth(1), Some("never opened"));
        assert_eq!(list[1].scope.tag(), "read only");
        // The migrated reader token wears the same tag as a read-only link
        // and sits in the same list under its own name.
        assert_eq!(list[2].name, "GrokBot-Arbos");
        assert_eq!(list[2].scope.tag(), "read only");
        assert!(
            list[2].subtitle.starts_with("Created 25 ")
                || list[2].subtitle.starts_with("Created 2"),
            "an older link shows its date, not a weekday: {}",
            list[2].subtitle
        );
        assert!(list[2].subtitle.ends_with(" · used 3 d ago"));
        // A label the relay sends empty still names something.
        let mut blank = link("z", "   ", "read");
        blank.label = "   ".into();
        assert_eq!(rows(&[blank], NOW)[0].name, "Agent link");
    }

    #[test]
    fn created_and_used_labels_step_from_weekdays_to_dates() {
        assert_eq!(created_label(NOW - 3_600_000, NOW).len(), 3, "a weekday");
        assert!(created_label(NOW - 40 * 24 * 3_600_000, NOW).contains(' '));
        assert_eq!(used_label(None, None, NOW), "never opened");
        assert_eq!(used_label(None, Some(NOW), NOW), "never opened");
        assert_eq!(
            used_label(Some(NOW - 30_000), Some(NOW - 30_000), NOW),
            "used just now"
        );
        assert_eq!(
            used_label(Some(NOW - 9 * 60_000), Some(NOW - 4 * 60_000), NOW),
            "used 4 min ago"
        );
        // Opened but never seen since: the open is the last thing it did.
        assert_eq!(
            used_label(Some(NOW - 2 * 3_600_000), None, NOW),
            "used 2 h ago"
        );
    }

    /// The copy row shows the host and the secret's tail, which is all that
    /// tells two copies apart; nothing is ever read back off the screen.
    #[test]
    fn the_copy_row_shortens_the_url_to_host_and_tail() {
        assert_eq!(
            short_url("https://relay.arbos.life/agent-link/open/Kq2vB7xZ9kT2vQ"),
            "relay.arbos.life/…/9kT2vQ"
        );
        assert_eq!(
            short_url("http://127.0.0.1:7700/agent-link/open/abc"),
            "127.0.0.1:7700/…/abc"
        );
        assert_eq!(short_url("relay.arbos.life"), "relay.arbos.life");
    }

    /// No two live links share a name, so the list stays readable whether or
    /// not the owner typed one.
    #[test]
    fn a_new_link_is_named_past_the_live_ones() {
        assert_eq!(next_label(&[]), "Agent link");
        let first = link("a", "Agent link", "read");
        assert_eq!(next_label(std::slice::from_ref(&first)), "Agent link 2");
        let second = link("b", "Agent link 2", "read");
        assert_eq!(next_label(&[first.clone(), second.clone()]), "Agent link 3");
        // A name freed by a revoke is free again.
        let mut gone = second;
        gone.live = false;
        assert_eq!(next_label(&[first, gone]), "Agent link 2");
        // A link the owner named does not push the count along.
        assert_eq!(
            next_label(&[link("c", "Jacob's Claude", "reply")]),
            "Agent link"
        );
    }

    /// The relay stores the one literal "Agent link" for an unlabelled link
    /// and never numbers it, so the name that goes on the wire is composed
    /// here and is never empty.
    #[test]
    fn the_name_we_send_is_ours_to_compose_and_never_blank() {
        let live = [link("a", "Agent link", "read")];
        assert_eq!(label_for("", &live), "Agent link 2");
        assert_eq!(label_for("   ", &live), "Agent link 2");
        assert_eq!(label_for("\t\n", &[]), "Agent link");
        // A typed name is kept, trimmed, and never reaches the relay over its limit.
        assert_eq!(label_for("  Jacob's Claude  ", &live), "Jacob's Claude");
        let long = "x".repeat(LABEL_MAX + 40);
        assert_eq!(label_for(&long, &live).chars().count(), LABEL_MAX);
        for typed in ["", " ", "Codex", &long] {
            assert!(!label_for(typed, &live).is_empty());
        }
    }

    #[test]
    fn revoking_takes_the_row_with_it() {
        let mut list = vec![
            link("a", "Jacob's Claude", "reply"),
            link("b", "Codex debugger", "read"),
        ];
        assert!(remove(&mut list, "a"));
        assert_eq!(list.iter().map(|l| l.id.clone()).collect::<Vec<_>>(), ["b"]);
        assert!(!remove(&mut list, "a"));
    }
}
