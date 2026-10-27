//! Inline mention chips (Jacob, 2026-10-23, after Grok Bot): when a bubble
//! says `@comms`, `@sam`, `@launch-week`, `@launch-crew`, `@here` or `@agents`
//! and we know who that is, the mention draws as a chip — the entity's picture,
//! its name in the accent colour, on a soft pill — and a click opens that
//! entity in the right pane (the shared D93 profile pane). Collective `@here`
//! / `@agents` chips have no page. Unknown handles stay plain text.
//!
//! How it fits the selectable prose (#1389): the text itself is not broken
//! into elements. [`Mentions::display`] rewrites `@comms` to `@Comms` (the
//! display name) before the bubble's plain text is built, so selection and
//! ⌘C still work across a chip and copy `@Comms`; [`Mentions::find`] locates
//! those `@Name` runs in a rendered text; `select::SelectableText` then paints
//! the pill behind the run, hides the `@` glyph and paints the picture in its
//! cell, and routes a plain click inside the run to [`Mention::page`].

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Entity, Hsla, RenderImage};

use crate::app::{AppState, ProfilePage};
use crate::model::Project;
use crate::ui::avatars::{self, LeafPick};
use crate::ui::theme::Theme;

/// The chip's geometry, Grok Bot's mention chip (Jacob's shot, Oct 23): the
/// pill hugs the text line with `PILL_PAD_X` on each side and `PILL_PAD_Y`
/// above and below the glyph box; the picture is a disc `PICTURE` tall
/// centred in the `@` glyph's cell.
pub const PILL_PAD_X: f32 = 4.0;
pub const PILL_PAD_Y: f32 = 1.0;
pub const PILL_RADIUS: f32 = 6.0;
/// The picture disc, at most; it shrinks to its cell so it never touches
/// the name (Grok: a clear gap between icon and name).
pub const PICTURE: f32 = 15.0;
/// The gap the disc keeps to the name, inside the cell.
pub const PICTURE_GAP: f32 = 3.0;
/// What the rewrite puts after `@`: a three-per-em space, so the picture
/// cell is `@` + ⅓ em wide — room for a 15 pt disc and the gap at body size,
/// a smaller disc at smaller sizes. Copies carry it as a thin space; `find`
/// expects it.
pub const CELL: &str = "@\u{2004}";

/// The pill's accent alpha at rest, under the pointer and while pressed
/// (Jacob, Oct 24: a chip must look clickable — brighter under the hand,
/// brighter still while the button is down).
pub const PILL_ALPHA: f32 = 0.14;
pub const PILL_ALPHA_HOVER: f32 = 0.22;
pub const PILL_ALPHA_PRESSED: f32 = 0.30;
/// How long the pill takes to fade between those (ease-out).
pub const PILL_FADE: Duration = Duration::from_millis(120);

/// A chip's pill colour: the accent at `alpha`.
pub fn pill_color(accent: Hsla, alpha: f32) -> Hsla {
    Theme::with_alpha(accent, alpha)
}

/// What the pointer is doing to a chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipState {
    Rest,
    Hover,
    Pressed,
}

impl ChipState {
    /// `hover` / `pressed` (the screenshot rig's `ARBOS_RIG_CHIP`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "rest" => Some(ChipState::Rest),
            "hover" => Some(ChipState::Hover),
            "pressed" => Some(ChipState::Pressed),
            _ => None,
        }
    }

    pub fn alpha(self) -> f32 {
        match self {
            ChipState::Rest => PILL_ALPHA,
            ChipState::Hover => PILL_ALPHA_HOVER,
            ChipState::Pressed => PILL_ALPHA_PRESSED,
        }
    }
}

/// The pill alpha `elapsed` into a fade from `from` toward `to` — ease-out,
/// done after [`PILL_FADE`].
pub fn pill_alpha(from: f32, to: ChipState, elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / PILL_FADE.as_secs_f32()).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    from + (to.alpha() - from) * eased
}

/// One chip the pointer touched: its key, the state it is fading toward,
/// and where the fade started.
#[derive(Clone, Debug)]
struct Touched<K> {
    key: K,
    state: ChipState,
    from: f32,
    since: Instant,
}

impl<K> Touched<K> {
    fn alpha(&self, now: Instant) -> f32 {
        pill_alpha(
            self.from,
            self.state,
            now.saturating_duration_since(self.since),
        )
    }

    fn settled(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) >= PILL_FADE
    }
}

/// The pointer over one surface's chips (a transcript's bubbles, the
/// composer): at most one chip is hot (hovered or pressed) and at most one is
/// cooling back to rest. `K` names a chip stably across frames. Every change
/// reports `true` so the caller can ask for a repaint; the surface reads the
/// alpha for each chip it paints with [`ChipPointer::alpha`].
#[derive(Clone, Debug)]
pub struct ChipPointer<K> {
    hot: Option<Touched<K>>,
    cooling: Option<Touched<K>>,
}

impl<K> Default for ChipPointer<K> {
    fn default() -> Self {
        Self {
            hot: None,
            cooling: None,
        }
    }
}

impl<K: Clone + PartialEq> ChipPointer<K> {
    /// The pointer is over `key` (or over no chip). A pressed chip stays
    /// pressed while the pointer is still on it.
    pub fn hover(&mut self, key: Option<K>, now: Instant) -> bool {
        if self.hot.as_ref().map(|t| &t.key) == key.as_ref() {
            return false;
        }
        if let Some(t) = self.hot.take() {
            self.cooling = Some(Touched {
                from: t.alpha(now),
                state: ChipState::Rest,
                since: now,
                key: t.key,
            });
        }
        if let Some(key) = key {
            // Back onto a chip still cooling: pick the fade up where it is.
            let from = match self.cooling.take_if(|c| c.key == key) {
                Some(c) => c.alpha(now),
                None => PILL_ALPHA,
            };
            self.hot = Some(Touched {
                key,
                state: ChipState::Hover,
                from,
                since: now,
            });
        }
        true
    }

    /// The button went down on `key`.
    pub fn press(&mut self, key: K, now: Instant) -> bool {
        self.hover(Some(key), now);
        self.set_hot_state(ChipState::Pressed, now)
    }

    /// The button came up: a pressed chip goes back to hover. Returns the
    /// key that was pressed, if any.
    pub fn release(&mut self, now: Instant) -> Option<K> {
        let pressed = self.pressed().cloned();
        self.set_hot_state(ChipState::Hover, now);
        pressed
    }

    fn set_hot_state(&mut self, state: ChipState, now: Instant) -> bool {
        let Some(hot) = self.hot.as_mut() else {
            return false;
        };
        if hot.state == state {
            return false;
        }
        hot.from = hot.alpha(now);
        hot.state = state;
        hot.since = now;
        true
    }

    /// The chip under the pointer, if any.
    pub fn hot(&self) -> Option<&K> {
        self.hot.as_ref().map(|t| &t.key)
    }

    /// The chip the button is down on, if any.
    pub fn pressed(&self) -> Option<&K> {
        self.hot
            .as_ref()
            .filter(|t| t.state == ChipState::Pressed)
            .map(|t| &t.key)
    }

    /// `key`'s state right now.
    #[cfg(test)]
    pub fn state(&self, key: &K) -> ChipState {
        match &self.hot {
            Some(t) if t.key == *key => t.state,
            _ => ChipState::Rest,
        }
    }

    /// `key`'s pill alpha at `now` and whether it is still fading (the
    /// caller should paint again next frame). `snap` skips the fade (reduced
    /// motion).
    pub fn alpha(&self, key: &K, now: Instant, snap: bool) -> (f32, bool) {
        let touched = [self.hot.as_ref(), self.cooling.as_ref()]
            .into_iter()
            .flatten()
            .find(|t| t.key == *key);
        match touched {
            None => (PILL_ALPHA, false),
            Some(t) if snap || t.settled(now) => (t.state.alpha(), false),
            Some(t) => (t.alpha(now), true),
        }
    }

    /// Drop a cooling chip once it has settled; call before painting a frame.
    pub fn tidy(&mut self, now: Instant) {
        self.cooling.take_if(|c| c.settled(now));
    }
}

/// What the chip paints in the `@` cell.
#[derive(Clone, Debug)]
pub enum Picture {
    Image(Arc<RenderImage>),
    /// D79 initials disc: two letters on the id's gradient.
    Initials {
        text: String,
        top: Hsla,
        bottom: Hsla,
    },
    /// D92 leaf disc.
    Leaf(LeafPick),
    /// The neutral disc (Jacob, 2026-10-25): the name's first letter on plain grey.
    Neutral {
        name: String,
    },
}

pub struct Mention {
    /// Where a click goes; `None` for collective `@here` / `@agents` (no profile).
    pub page: Option<ProfilePage>,
    /// The display name the chip shows (and the text carries after `@`).
    pub name: String,
    pub picture: Picture,
    /// The member id a sent row carries for this entity (`user:<id>` /
    /// `agent:<project-id>`), or the collective marker (`here` / `agents`)
    /// the sources expand into the chat's members; `None` for groups and
    /// servers.
    pub id: Option<String>,
}

/// What a composer chip stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// One person or agent (`user:<id>` / `agent:<project-id>`).
    Member,
    /// `@here` (everyone in this chat) or `@agents` (all agents here).
    Collective,
}

/// A committed mention in the composer (#1596): the chip stands for one
/// entity, picked from the menu or resolved from an exact `@handle`. Its text
/// in the field is [`CELL`] + `name` at `range`; the sent row carries
/// `@handle` in the text and `id` in `mentions` (a collective's marker is
/// expanded into member ids by the source).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionToken {
    pub range: Range<usize>,
    pub handle: String,
    pub name: String,
    pub id: Option<String>,
    pub kind: TokenKind,
}

impl MentionToken {
    /// What the token reads as in the sent text.
    pub fn plain(&self) -> String {
        format!("@{}", self.handle)
    }

    pub fn kind_of(handle: &str) -> TokenKind {
        if crate::sync::is_collective(handle) {
            TokenKind::Collective
        } else {
            TokenKind::Member
        }
    }
}

/// Everyone a bubble can mention, keyed by handle (lower-case), plus the
/// state handle a click updates.
pub struct Mentions {
    by_handle: HashMap<String, Rc<Mention>>,
    /// The same entities by member id, for a composer token that knows who
    /// it is even when its handle is shared (#1538).
    by_id: HashMap<String, Rc<Mention>>,
    /// Handles two different entities carry (an agent and a person, #1538):
    /// a typed exact handle is not auto-resolved for these — the picker or
    /// the send-time tie rule decides.
    ambiguous: HashSet<String>,
    /// Display names, longest first, so `@Launch week` beats `@Launch`.
    names: Vec<(String, Rc<Mention>)>,
    /// What a click does with the page: open the right pane (the app), or
    /// record it (tests).
    opener: Option<Opener>,
}

type Opener = Rc<dyn Fn(&ProfilePage, &mut App)>;

impl Mentions {
    /// Nothing resolves (tests, previews).
    pub fn empty() -> Rc<Self> {
        Rc::new(Self {
            by_handle: HashMap::new(),
            by_id: HashMap::new(),
            ambiguous: HashSet::new(),
            names: Vec::new(),
            opener: None,
        })
    }

    /// A fixed table with a recording opener (tests, previews).
    #[cfg(test)]
    pub fn with_table(
        entries: Vec<(&str, Mention)>,
        opener: impl Fn(&ProfilePage, &mut App) + 'static,
    ) -> Rc<Self> {
        let mut table = Table::default();
        for (handle, m) in entries {
            table.add(handle.to_string(), m);
        }
        Rc::new(table.finish(Some(Rc::new(opener))))
    }

    /// People, agents, groups and servers the app knows right now.
    pub fn from_state(state: &Entity<AppState>, theme: &Theme, cx: &App) -> Rc<Self> {
        let s = state.read(cx);
        let mut table = Table::default();
        let mut add = |handle: String, m: Mention| table.add(handle, m);
        // Collective pings first so a rare handle collision cannot steal them.
        // Their id is the marker the sources expand (`sync::expand_collectives`).
        add(
            "here".into(),
            Mention {
                page: None,
                name: "here".into(),
                picture: initials("here", "here", None, theme),
                id: Some(crate::sync::COLLECTIVE_HERE.into()),
            },
        );
        add(
            "agents".into(),
            Mention {
                page: None,
                name: "agents".into(),
                picture: initials("agents", "agents", None, theme),
                id: Some(crate::sync::COLLECTIVE_AGENTS.into()),
            },
        );
        for p in &s.projects {
            if p.is_people() && !p.is_people_group() {
                // A DM: its person is mentionable by handle.
                if let Some(person) = p.person() {
                    add(
                        person.handle.clone(),
                        Mention {
                            page: Some(ProfilePage::User {
                                handle: person.handle.clone(),
                                space_id: Some(p.id.clone()),
                            }),
                            name: person.display_name(),
                            picture: person_picture(s, person, theme),
                            id: Some(format!("user:{}", person.id)),
                        },
                    );
                }
                continue;
            }
            let slug = Project::mention_slug(&p.name);
            if slug.is_empty() {
                continue;
            }
            let (page, id) = if p.is_people_group() {
                (
                    ProfilePage::Group {
                        space_id: p.id.clone(),
                    },
                    None,
                )
            } else {
                (
                    ProfilePage::Agent {
                        project_id: p.id.clone(),
                    },
                    Some(format!("agent:{}", p.id)),
                )
            };
            add(
                slug,
                Mention {
                    page: Some(page),
                    name: p.name.trim().to_string(),
                    picture: project_picture(s, p, theme),
                    id,
                },
            );
        }
        // Agents other people shared into my chats (#1592): their `@handle` is a chip
        // too, drawn through the same identity the bubble and the presence row use.
        for space in s.projects.iter().filter(|p| p.is_people()) {
            for a in &space.agents {
                let ident = avatars::member_identity(Some(space), &s.projects, a);
                if ident.handle.is_empty() {
                    continue;
                }
                add(
                    ident.handle.clone(),
                    Mention {
                        page: Some(ProfilePage::Agent {
                            project_id: ident.id.clone(),
                        }),
                        name: ident.name.clone(),
                        picture: identity_picture(s, &ident, theme),
                        id: Some(format!("agent:{}", ident.id)),
                    },
                );
            }
        }
        // People who sit in groups (and the search results) by handle.
        for person in s
            .projects
            .iter()
            .flat_map(|p| p.people.iter())
            .chain(s.people_results.iter())
        {
            add(
                person.handle.clone(),
                Mention {
                    page: Some(ProfilePage::User {
                        handle: person.handle.clone(),
                        space_id: None,
                    }),
                    name: person.display_name(),
                    picture: person_picture(s, person, theme),
                    id: Some(format!("user:{}", person.id)),
                },
            );
        }
        for server in &s.servers {
            let slug = Project::mention_slug(&server.name);
            if slug.is_empty() {
                continue;
            }
            let picture = match (
                avatars::leaf_avatars_on(),
                server.avatar_leaf,
                server.avatar_leaf_color,
            ) {
                (_, Some(avatars::leaf::NEUTRAL), _) => Picture::Neutral {
                    name: server.name.clone(),
                },
                (true, Some(leaf), Some(color)) => Picture::Leaf(LeafPick { leaf, color }),
                _ => initials(
                    &server.id,
                    &server.name,
                    server.avatar_color_index_effective,
                    theme,
                ),
            };
            add(
                slug,
                Mention {
                    page: Some(ProfilePage::Server {
                        server_id: server.id.clone(),
                    }),
                    name: server.name.trim().to_string(),
                    picture,
                    id: None,
                },
            );
        }
        // Me, by id only (never a chip for my own handle): membership lines name their
        // actor by user id from the live roster, and I am the actor of my own adds.
        let me_id = s.me_id();
        if !me_id.is_empty() {
            table
                .by_id
                .entry(format!("user:{me_id}"))
                .or_insert_with(|| {
                    Rc::new(Mention {
                        page: None,
                        name: s.me_name(),
                        picture: initials(&me_id, &s.me_name(), None, theme),
                        id: Some(format!("user:{me_id}")),
                    })
                });
        }
        // A chip is a profile target like the sender name beside it: open,
        // close on the second click, swap for someone else
        // ([feedback profile-pane-toggle-mac-2026-10-26]).
        let state = state.clone();
        Rc::new(
            table.finish(Some(Rc::new(move |page: &ProfilePage, cx: &mut App| {
                let page = page.clone();
                state.update(cx, |s, cx| s.toggle_profile(page, cx));
            }))),
        )
    }

    /// The entity behind a handle (case-insensitive), if known.
    pub fn by_handle(&self, handle: &str) -> Option<&Rc<Mention>> {
        self.by_handle.get(&handle.to_lowercase())
    }

    /// The entity behind a member id (`user:<id>` / `agent:<project-id>`).
    pub fn by_id(&self, id: &str) -> Option<&Rc<Mention>> {
        self.by_id.get(id)
    }

    /// The live display name of a `user` / `agent` by id, for a membership line.
    pub fn name_of(&self, kind: &str, id: &str) -> Option<String> {
        self.by_id(&format!("{kind}:{id}"))
            .map(|m| m.name.trim().to_owned())
            .filter(|n| !n.is_empty())
    }

    /// True when two different entities carry `handle` (#1538): a typed exact
    /// handle stays plain text until the picker or the send-time rule decides.
    pub fn is_ambiguous(&self, handle: &str) -> bool {
        self.ambiguous.contains(&handle.to_lowercase())
    }

    /// The entity a composer token stands for (its picture, its page): by id
    /// first (exact entity), else by handle.
    pub fn mention_for(&self, token: &MentionToken) -> Option<&Rc<Mention>> {
        token
            .id
            .as_deref()
            .and_then(|id| self.by_id(id))
            .or_else(|| self.by_handle(&token.handle))
    }

    /// `@handle` → `@Display Name` for every handle we know; anything else is
    /// left exactly as typed (an unknown `@someone` stays plain text).
    pub fn display(&self, text: &str) -> String {
        if self.by_handle.is_empty() || !text.contains('@') {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find('@') {
            let (before, from_at) = rest.split_at(at);
            out.push_str(before);
            let starts_token = before
                .chars()
                .last()
                .is_none_or(|c| !c.is_alphanumeric() && c != '@');
            let handle_len = handle_len(&from_at[1..]);
            if starts_token && handle_len > 0 {
                let handle = &from_at[1..1 + handle_len];
                if let Some(m) = self.by_handle.get(&handle.to_lowercase()) {
                    out.push_str(CELL);
                    out.push_str(&m.name);
                    rest = &from_at[1 + handle_len..];
                    continue;
                }
            }
            out.push('@');
            rest = &from_at[1..];
        }
        out.push_str(rest);
        out
    }

    /// The `@Name` runs in a displayed text (byte ranges include the `@`).
    pub fn find(&self, text: &str) -> Vec<(Range<usize>, Rc<Mention>)> {
        let mut out = Vec::new();
        if self.names.is_empty() {
            return out;
        }
        let mut i = 0;
        while let Some(off) = text[i..].find('@') {
            let at = i + off;
            let starts_token = text[..at]
                .chars()
                .last()
                .is_none_or(|c| !c.is_alphanumeric() && c != '@');
            // The rewrite leaves `@` + hair space before a resolved name; a
            // plain `@Name` typed by hand also counts.
            let cell = if text[at..].starts_with(CELL) {
                CELL.len()
            } else {
                1
            };
            let after = &text[at + cell..];
            let hit = starts_token
                .then(|| {
                    self.names.iter().find(|(name, _)| {
                        after.starts_with(name.as_str())
                            && after[name.len()..]
                                .chars()
                                .next()
                                .is_none_or(|c| !c.is_alphanumeric())
                    })
                })
                .flatten();
            match hit {
                Some((name, m)) => {
                    out.push((at..at + cell + name.len(), m.clone()));
                    i = at + cell + name.len();
                }
                None => i = at + 1,
            }
        }
        out
    }

    /// `@handle` runs in raw typed text (composer): no display rewrite, the
    /// `@` stays visible. Byte ranges cover `@` + the handle.
    pub fn find_raw(&self, text: &str) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        if self.by_handle.is_empty() || !text.contains('@') {
            return out;
        }
        let mut i = 0;
        while let Some(off) = text[i..].find('@') {
            let at = i + off;
            let starts_token = text[..at]
                .chars()
                .last()
                .is_none_or(|c| !c.is_alphanumeric() && c != '@');
            let handle_len = handle_len(&text[at + 1..]);
            if starts_token && handle_len > 0 {
                let handle = &text[at + 1..at + 1 + handle_len];
                if self.by_handle.contains_key(&handle.to_lowercase()) {
                    out.push(at..at + 1 + handle_len);
                    i = at + 1 + handle_len;
                    continue;
                }
            }
            i = at + 1;
        }
        out
    }

    /// Open the mention's page in the right pane (no-op for `@here` / `@agents`).
    pub fn open(&self, m: &Mention, cx: &mut App) {
        let (Some(open), Some(page)) = (&self.opener, &m.page) else {
            return;
        };
        open(page, cx);
    }
}

/// The table under construction: first entity per handle wins (bubbles), every
/// entity is reachable by id (composer tokens), and a handle two kinds of
/// entity share is remembered as ambiguous (#1538).
#[derive(Default)]
struct Table {
    by_handle: HashMap<String, Rc<Mention>>,
    by_id: HashMap<String, Rc<Mention>>,
    ambiguous: HashSet<String>,
}

impl Table {
    fn add(&mut self, handle: String, m: Mention) {
        let key = handle.to_lowercase();
        let m = Rc::new(m);
        if let Some(id) = m.id.as_deref() {
            self.by_id
                .entry(id.to_string())
                .or_insert_with(|| m.clone());
        }
        match self.by_handle.get(&key) {
            None => {
                self.by_handle.insert(key, m);
            }
            Some(first) => {
                let first_is_agent = first.id.as_deref().is_some_and(|i| i.starts_with("agent:"));
                let this_is_agent = m.id.as_deref().is_some_and(|i| i.starts_with("agent:"));
                if first.id.is_some() && m.id.is_some() && first_is_agent != this_is_agent {
                    self.ambiguous.insert(key);
                }
            }
        }
    }

    fn finish(self, opener: Option<Opener>) -> Mentions {
        let names = sorted_names(&self.by_handle);
        Mentions {
            by_handle: self.by_handle,
            by_id: self.by_id,
            ambiguous: self.ambiguous,
            names,
            opener,
        }
    }
}

/// Display names, longest first (so `@Launch week` beats `@Launch`), one
/// entry per name.
fn sorted_names(by_handle: &HashMap<String, Rc<Mention>>) -> Vec<(String, Rc<Mention>)> {
    let mut names: Vec<(String, Rc<Mention>)> = by_handle
        .values()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();
    names.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    names.dedup_by(|a, b| a.0 == b.0);
    names
}

/// `[A-Za-z0-9_.-]+` with trailing `.`/`-` punctuation left out of the handle.
fn handle_len(s: &str) -> usize {
    let mut len = 0;
    for (i, c) in s.char_indices() {
        if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
            len = i + c.len_utf8();
        } else {
            break;
        }
    }
    while len > 0 && matches!(s.as_bytes()[len - 1], b'.' | b'-') {
        len -= 1;
    }
    len
}

/// A D79 initials disc for `name`, coloured from `seed` (or `index`).
pub fn initials_picture(seed: &str, name: &str, index: Option<u8>, theme: &Theme) -> Picture {
    initials(seed, name, index, theme)
}

fn initials(seed: &str, name: &str, index: Option<u8>, theme: &Theme) -> Picture {
    let (top, bottom) = avatars::gradient(avatars::index_for(seed, index), theme);
    Picture::Initials {
        text: avatars::initials(name),
        top,
        bottom,
    }
}

/// An agent member of a people chat (`space`), through the same identity rule
/// its bubbles and the presence row use (#1592).
pub fn agent_member_picture(
    s: &AppState,
    space: Option<&Project>,
    a: &crate::model::AgentMember,
    theme: &Theme,
) -> Picture {
    let ident = avatars::member_identity(space, &s.projects, a);
    identity_picture(s, &ident, theme)
}

pub fn person_picture(s: &AppState, person: &crate::model::Person, theme: &Theme) -> Picture {
    match s.avatar(&person.avatar_url) {
        Some(image) => Picture::Image(image),
        None => initials(
            avatars::person_seed(person),
            &person.display_name(),
            person.avatar_color_index,
            theme,
        ),
    }
}

/// The chip picture for a resolved agent: its uploaded picture, else its leaf (D92) or
/// initials (D79) — the identity's own picks, never a guess from the handle.
fn identity_picture(s: &AppState, a: &avatars::AgentIdentity, theme: &Theme) -> Picture {
    if let Some(image) = a.image_key.as_deref().and_then(|url| s.avatar(url)) {
        return Picture::Image(image);
    }
    if a.neutral {
        return Picture::Neutral {
            name: a.name.clone(),
        };
    }
    if avatars::leaf_avatars_on() {
        Picture::Leaf(a.leaf)
    } else {
        initials(&a.id, &a.name, Some(a.color_index), theme)
    }
}

fn project_picture(s: &AppState, p: &Project, theme: &Theme) -> Picture {
    if let Some(image) = p.image_url.as_deref().and_then(|url| s.avatar(url)) {
        return Picture::Image(image);
    }
    if avatars::leaf::is_neutral(p.avatar_leaf) {
        return Picture::Neutral {
            name: p.name.clone(),
        };
    }
    if p.is_people_group() {
        return match (
            avatars::leaf_avatars_on(),
            p.avatar_leaf,
            p.avatar_leaf_color,
        ) {
            (true, Some(leaf), Some(color)) => Picture::Leaf(LeafPick { leaf, color }),
            _ => initials(&p.id, &p.name, p.avatar_color_index, theme),
        };
    }
    if avatars::leaf_avatars_on() {
        Picture::Leaf(avatars::project_leaf(p))
    } else {
        initials(&p.id, &p.name, p.avatar_color_index, theme)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn stub(name: &str, page: ProfilePage) -> Mention {
        let id = match &page {
            ProfilePage::Agent { project_id } => Some(format!("agent:{project_id}")),
            ProfilePage::User { handle, .. } => Some(format!("user:gh_{handle}")),
            _ => None,
        };
        Mention {
            page: Some(page),
            name: name.to_string(),
            picture: Picture::Initials {
                text: "X".into(),
                top: Hsla::default(),
                bottom: Hsla::default(),
            },
            id,
        }
    }

    fn mentions(entries: &[(&str, &str, ProfilePage)]) -> Rc<Mentions> {
        Mentions::with_table(
            entries
                .iter()
                .map(|(h, n, p)| (*h, stub(n, p.clone())))
                .collect(),
            |_, _| {},
        )
    }

    fn four() -> Rc<Mentions> {
        mentions(&[
            (
                "comms",
                "Comms",
                ProfilePage::Agent {
                    project_id: "p-comms".into(),
                },
            ),
            (
                "sam",
                "Sam Rivera",
                ProfilePage::User {
                    handle: "sam".into(),
                    space_id: None,
                },
            ),
            (
                "launch-week",
                "Launch week",
                ProfilePage::Group {
                    space_id: "g-lw".into(),
                },
            ),
            (
                "launch-crew",
                "Launch Crew",
                ProfilePage::Server {
                    server_id: "s-lc".into(),
                },
            ),
        ])
    }

    #[test]
    fn every_entity_kind_becomes_a_chip_and_opens_its_page() {
        let m = four();
        let text = m.display("ask @comms, @sam, @launch-week and @Launch-Crew.");
        assert_eq!(
            text,
            format!("ask {CELL}Comms, {CELL}Sam Rivera, {CELL}Launch week and {CELL}Launch Crew.")
        );
        let chips = m.find(&text);
        let pages: Vec<&ProfilePage> = chips.iter().filter_map(|(_, m)| m.page.as_ref()).collect();
        assert_eq!(chips.len(), 4);
        assert!(matches!(pages[0], ProfilePage::Agent { project_id } if project_id == "p-comms"));
        assert!(matches!(pages[1], ProfilePage::User { handle, .. } if handle == "sam"));
        assert!(matches!(pages[2], ProfilePage::Group { space_id } if space_id == "g-lw"));
        assert!(matches!(pages[3], ProfilePage::Server { server_id } if server_id == "s-lc"));
        // Each range covers "@" + the display name, and nothing else.
        for (range, mention) in &chips {
            assert_eq!(&text[range.clone()], format!("{CELL}{}", mention.name));
        }
    }

    #[test]
    fn here_and_agents_become_chips_without_a_page() {
        let m = Mentions::with_table(
            vec![
                (
                    "here",
                    Mention {
                        page: None,
                        name: "here".into(),
                        picture: Picture::Initials {
                            text: "H".into(),
                            top: Hsla::default(),
                            bottom: Hsla::default(),
                        },
                        id: None,
                    },
                ),
                (
                    "agents",
                    Mention {
                        page: None,
                        name: "agents".into(),
                        picture: Picture::Initials {
                            text: "A".into(),
                            top: Hsla::default(),
                            bottom: Hsla::default(),
                        },
                        id: None,
                    },
                ),
            ],
            |_, _| {},
        );
        let text = m.display("ping @here and @agents please");
        assert_eq!(text, format!("ping {CELL}here and {CELL}agents please"));
        let chips = m.find(&text);
        assert_eq!(chips.len(), 2);
        assert!(chips[0].1.page.is_none());
        assert!(chips[1].1.page.is_none());
        assert_eq!(
            m.find_raw("ping @here and @agents please"),
            vec![5..10, 15..22]
        );
        // Partial tokens stay plain.
        assert!(m.find_raw("@he @agent").is_empty());
    }

    #[test]
    fn unknown_handles_stay_plain_text() {
        let m = four();
        let text = "ping @nobody and @comms_old or email a@b.com";
        assert_eq!(m.display(text), text);
        assert!(m.find(&m.display(text)).is_empty());
        // An email's `@` is glued to a word: never a mention.
        assert!(m.find("mail jacob@comms").is_empty());
    }

    #[test]
    fn trailing_punctuation_and_case_do_not_matter() {
        let m = four();
        assert_eq!(m.display("(@Comms)."), format!("({CELL}Comms)."));
        assert_eq!(m.display("@sam."), format!("{CELL}Sam Rivera."));
        assert_eq!(m.display("@SAM!"), format!("{CELL}Sam Rivera!"));
        // Typed by hand, no hair space: still a chip.
        let chips = m.find("@Sam Rivera!");
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].0, 0.."@Sam Rivera".len());
    }

    #[test]
    fn the_longest_name_wins_and_names_are_not_re_resolved() {
        let m = mentions(&[
            (
                "launch",
                "Launch",
                ProfilePage::Agent {
                    project_id: "a".into(),
                },
            ),
            (
                "launch-week",
                "Launch week",
                ProfilePage::Group {
                    space_id: "g".into(),
                },
            ),
        ]);
        let text = m.display("@launch-week and @launch");
        assert_eq!(text, format!("{CELL}Launch week and {CELL}Launch"));
        let chips = m.find(&text);
        assert_eq!(chips.len(), 2);
        assert_eq!(&text[chips[0].0.clone()], format!("{CELL}Launch week"));
        assert_eq!(&text[chips[1].0.clone()], format!("{CELL}Launch"));
    }

    #[test]
    fn ids_index_every_entity_and_a_shared_handle_is_ambiguous() {
        // `@arbos` is both an agent and a person (#1538): the bubble keeps the
        // first (the agent), a composer token can still name the person by id,
        // and a typed exact `@arbos` is not auto-resolved.
        let m = mentions(&[
            (
                "arbos",
                "Arbos",
                ProfilePage::Agent {
                    project_id: "p-arbos".into(),
                },
            ),
            (
                "arbos",
                "Jacob",
                ProfilePage::User {
                    handle: "arbos".into(),
                    space_id: None,
                },
            ),
            (
                "sam",
                "Sam Rivera",
                ProfilePage::User {
                    handle: "sam".into(),
                    space_id: None,
                },
            ),
        ]);
        assert_eq!(m.by_handle("ARBOS").unwrap().name, "Arbos");
        assert_eq!(m.by_id("user:gh_arbos").unwrap().name, "Jacob");
        assert_eq!(m.by_id("agent:p-arbos").unwrap().name, "Arbos");
        assert!(m.is_ambiguous("arbos"));
        assert!(!m.is_ambiguous("sam"));
        let token = MentionToken {
            range: 0..0,
            handle: "arbos".into(),
            name: "Jacob".into(),
            id: Some("user:gh_arbos".into()),
            kind: TokenKind::Member,
        };
        assert_eq!(m.mention_for(&token).unwrap().name, "Jacob");
        assert_eq!(token.plain(), "@arbos");
    }

    #[test]
    fn the_pointer_fades_a_chip_between_rest_hover_and_pressed() {
        let t0 = Instant::now();
        let half = PILL_FADE / 2;
        let mut p: ChipPointer<u8> = ChipPointer::default();
        assert_eq!(p.alpha(&1, t0, false), (PILL_ALPHA, false));
        assert!(!p.hover(None, t0), "nothing to nothing is no change");

        assert!(p.hover(Some(1), t0));
        assert!(!p.hover(Some(1), t0), "staying put is no change");
        assert_eq!(p.state(&1), ChipState::Hover);
        let (mid, fading) = p.alpha(&1, t0 + half, false);
        assert!(
            fading && mid > PILL_ALPHA && mid < PILL_ALPHA_HOVER,
            "{mid}"
        );
        assert_eq!(
            p.alpha(&1, t0 + PILL_FADE, false),
            (PILL_ALPHA_HOVER, false)
        );
        assert_eq!(
            p.alpha(&1, t0, true),
            (PILL_ALPHA_HOVER, false),
            "reduced motion: no fade"
        );
        assert_eq!(
            p.alpha(&2, t0 + half, false),
            (PILL_ALPHA, false),
            "others rest"
        );

        assert!(p.press(1, t0 + PILL_FADE));
        assert_eq!(p.state(&1), ChipState::Pressed);
        assert_eq!(p.pressed(), Some(&1));
        assert_eq!(
            p.alpha(&1, t0 + PILL_FADE * 2, false),
            (PILL_ALPHA_PRESSED, false)
        );
        assert_eq!(p.release(t0 + PILL_FADE * 2), Some(1));
        assert_eq!(p.state(&1), ChipState::Hover);
        assert_eq!(p.release(t0 + PILL_FADE * 2), None, "nothing pressed now");

        // Leaving: the chip cools from where it was back to rest.
        let t1 = t0 + PILL_FADE * 3;
        assert!(p.hover(None, t1));
        assert_eq!(p.hot(), None);
        assert_eq!(p.state(&1), ChipState::Rest);
        let (cooling, fading) = p.alpha(&1, t1 + half, false);
        assert!(fading && cooling > PILL_ALPHA && cooling < PILL_ALPHA_HOVER);
        // Back on it mid-fade: picks up from the cooling alpha, no jump.
        assert!(p.hover(Some(1), t1 + half));
        let (resumed, _) = p.alpha(&1, t1 + half, false);
        assert!((resumed - cooling).abs() < 1e-5, "{resumed} vs {cooling}");
        assert!(p.hover(None, t1 + PILL_FADE * 2));
        p.tidy(t1 + PILL_FADE * 4);
        assert_eq!(p.alpha(&1, t1 + PILL_FADE * 4, false), (PILL_ALPHA, false));

        // Hopping chip to chip: the old one cools while the new one warms.
        assert!(p.hover(Some(1), t1));
        assert!(p.hover(Some(2), t1 + PILL_FADE));
        assert_eq!(p.state(&2), ChipState::Hover);
        assert!(p.alpha(&1, t1 + PILL_FADE + half, false).1, "1 is cooling");
        assert!(p.alpha(&2, t1 + PILL_FADE + half, false).1, "2 is warming");
    }

    #[test]
    fn chip_state_parses_the_rig_spec() {
        assert_eq!(ChipState::parse("hover"), Some(ChipState::Hover));
        assert_eq!(ChipState::parse(" pressed "), Some(ChipState::Pressed));
        assert_eq!(ChipState::parse("rest"), Some(ChipState::Rest));
        assert_eq!(ChipState::parse("down"), None);
    }

    #[test]
    fn pill_alpha_eases_out() {
        assert_eq!(
            pill_alpha(PILL_ALPHA, ChipState::Hover, Duration::ZERO),
            PILL_ALPHA
        );
        assert_eq!(
            pill_alpha(PILL_ALPHA, ChipState::Hover, PILL_FADE * 2),
            PILL_ALPHA_HOVER
        );
        let mid = pill_alpha(PILL_ALPHA, ChipState::Hover, PILL_FADE / 2);
        let linear = (PILL_ALPHA + PILL_ALPHA_HOVER) / 2.0;
        assert!(mid > linear, "ease-out is ahead of linear at the midpoint");
        assert_eq!(
            pill_color(Hsla::default(), 0.3),
            Theme::with_alpha(Hsla::default(), 0.3)
        );
    }

    #[test]
    fn handle_len_stops_at_punctuation() {
        assert_eq!(handle_len("comms, next"), 5);
        assert_eq!(handle_len("launch-week."), 11);
        assert_eq!(handle_len("a.b-c"), 5);
        assert_eq!(handle_len(""), 0);
        assert_eq!(handle_len("-"), 0);
    }
}
