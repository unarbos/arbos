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

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

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

/// What the chip paints in the `@` cell.
#[derive(Clone)]
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
}

pub struct Mention {
    /// Where a click goes; `None` for collective `@here` / `@agents` (no profile).
    pub page: Option<ProfilePage>,
    /// The display name the chip shows (and the text carries after `@`).
    pub name: String,
    pub picture: Picture,
}

/// Everyone a bubble can mention, keyed by handle (lower-case), plus the
/// state handle a click updates.
pub struct Mentions {
    by_handle: HashMap<String, Rc<Mention>>,
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
        let mut by_handle = HashMap::new();
        for (handle, m) in entries {
            by_handle.insert(handle.to_lowercase(), Rc::new(m));
        }
        let names = sorted_names(&by_handle);
        Rc::new(Self {
            by_handle,
            names,
            opener: Some(Rc::new(opener)),
        })
    }

    /// People, agents, groups and servers the app knows right now.
    pub fn from_state(state: &Entity<AppState>, theme: &Theme, cx: &App) -> Rc<Self> {
        let s = state.read(cx);
        let mut by_handle: HashMap<String, Rc<Mention>> = HashMap::new();
        let mut add = |handle: String, m: Mention| {
            by_handle
                .entry(handle.to_lowercase())
                .or_insert_with(|| Rc::new(m));
        };
        // Collective pings first so a rare handle collision cannot steal them.
        add(
            "here".into(),
            Mention {
                page: None,
                name: "here".into(),
                picture: initials("here", "here", None, theme),
            },
        );
        add(
            "agents".into(),
            Mention {
                page: None,
                name: "agents".into(),
                picture: initials("agents", "agents", None, theme),
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
                        },
                    );
                }
                continue;
            }
            let slug = Project::mention_slug(&p.name);
            if slug.is_empty() {
                continue;
            }
            let page = if p.is_people_group() {
                ProfilePage::Group {
                    space_id: p.id.clone(),
                }
            } else {
                ProfilePage::Agent {
                    project_id: p.id.clone(),
                }
            };
            add(
                slug,
                Mention {
                    page: Some(page),
                    name: p.name.trim().to_string(),
                    picture: project_picture(s, p, theme),
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
                },
            );
        }
        let names = sorted_names(&by_handle);
        let state = state.clone();
        Rc::new(Self {
            by_handle,
            names,
            opener: Some(Rc::new(move |page: &ProfilePage, cx: &mut App| {
                let page = page.clone();
                state.update(cx, |s, cx| s.open_profile(page, cx));
            })),
        })
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

fn initials(seed: &str, name: &str, index: Option<u8>, theme: &Theme) -> Picture {
    let (top, bottom) = avatars::gradient(avatars::index_for(seed, index), theme);
    Picture::Initials {
        text: avatars::initials(name),
        top,
        bottom,
    }
}

fn person_picture(s: &AppState, person: &crate::model::Person, theme: &Theme) -> Picture {
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
        Mention {
            page: Some(page),
            name: name.to_string(),
            picture: Picture::Initials {
                text: "X".into(),
                top: Hsla::default(),
                bottom: Hsla::default(),
            },
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
    fn handle_len_stops_at_punctuation() {
        assert_eq!(handle_len("comms, next"), 5);
        assert_eq!(handle_len("launch-week."), 11);
        assert_eq!(handle_len("a.b-c"), 5);
        assert_eq!(handle_len(""), 0);
        assert_eq!(handle_len("-"), 0);
    }
}
