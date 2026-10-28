//! The one profile pane (D93, Jacob 2026-10-22): people, own profile, agents,
//! groups, servers and channels are all rendered by [`render`] from a
//! [`ProfilePane`] spec. The spec says *which* tabs exist and *which* rows sit
//! under each; this module owns the look — hero, tab pill, cards, spacing,
//! footer, the frame (×, ‹ back from the stack, Done) — so every entity looks
//! the same and the iPhone's `ProfilePane` mirrors it one to one. Rows come
//! from `kit` (`row`, `nav_row`, `value_row`, `toggle_row`, `text_row`,
//! `danger_row`, `confirm_row`), never laid out by hand in a page.
//! API: `docs/group-chat-redesign.md` § "Shared profile pane".

use gpui::{
    div, prelude::*, px, AnyElement, Bounds, Context, Div, Hsla, Pixels, SharedString, Stateful,
};

use crate::app::{AppState, ProfilePage};
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_TINY};

use super::{hero, kit, pushed_frame, tab_bar, HeroClick, HERO_AVATAR, PANE_FADE_BAND};

/// Everything a page declares. The component renders it.
pub struct ProfilePane {
    /// Identity: the navigation stack, [`is_open`], element ids.
    pub page: ProfilePage,
    pub hero: Hero,
    /// Same labels and order as the iPhone page; zero or one tab shows no pill.
    pub tabs: &'static [&'static str],
    /// `state.profile_tab` (the stack restores it when a pushed page pops).
    pub selected: usize,
    /// The selected tab's content, top to bottom.
    pub body: Vec<Section>,
    /// Pinned after the body: Block user, Leave group, Sign out…
    pub footer: Vec<Section>,
    /// Optional control beside the × (Done, Save).
    pub done: Option<Stateful<Div>>,
    /// Grok's share disc, left of the ×, when the page has something to share
    /// (an agent or a group: the invite sheet). `None` draws the × alone.
    pub share: Option<HeroClick>,
}

pub struct Hero {
    pub picture: Picture,
    /// `name(text)` for a read-only name, `editable_name(..)` when owned.
    pub name: AnyElement,
    /// Glyph after the name ("lock" on a private channel).
    pub badge: Option<&'static str>,
    pub subtitle: Option<SharedString>,
    pub subtitle_color: Hsla,
    /// A click on the subtitle (the agent's "Add a label": edit it in place).
    pub subtitle_click: Option<HeroClick>,
    /// A ready-made subtitle in place of the text (the foreign agent's
    /// "Run by …" link, [feedback agent-pane-run-by-link-mac-2026-10-25]).
    pub subtitle_element: Option<AnyElement>,
    /// Mac only: inline controls under the subtitle while the name is being
    /// edited (the own profile's handle field with Cancel / Save). The iPhone
    /// edits in a sheet, so its hero has no twin field.
    pub editor: Option<AnyElement>,
    /// The hero's one action pill under the name and handle (a person's
    /// "Message", [feedback profile-message-button-mac-2026-10-25]); `None`
    /// on panes with no primary action, and on a person you cannot message
    /// (yourself, someone you blocked).
    pub action: Option<AnyElement>,
}

pub enum Picture {
    /// A photo, initials disc or leaf — `editable_picture(..)` when owned.
    Element(AnyElement),
    /// A glyph tile: "#" for channels, "archive"… (the channel page, bc-55453a88).
    Glyph(&'static str),
}

// Every variant is part of the published API; a page uses the ones its tabs
// need (D93).
#[allow(dead_code)]
pub enum Section {
    /// Small caps title over a card ("Images · 4", "Archived").
    Title(SharedString),
    /// Rows from `kit` (each draws its own hung hairline). Plain content
    /// without a hairline uses `kit::section` instead.
    Card(Vec<AnyElement>),
    /// A card whose row `open` has just unfolded a picker under it (Model,
    /// Reasoning): the pane scrolls so that row is in view, once.
    Reveal {
        rows: Vec<AnyElement>,
        open: usize,
    },
    /// A picker card: a `kit` row that opens and closes it, the swatch grid
    /// under it (leaves, colours), a small note under the grid.
    Picker {
        header: AnyElement,
        swatches: AnyElement,
        note: SharedString,
    },
    /// Centred muted line for an empty tab.
    Empty(SharedString),
    Footnote(SharedString),
    /// Escape hatch — say why on the component's issue.
    Custom(AnyElement),
}

/// The read-only hero name.
#[allow(dead_code)]
pub fn name(text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    div()
        .text_size(px(22.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(theme.text)
        .truncate()
        .child(text.into())
        .into_any_element()
}

/// The page's frame title, for the pushed bar.
fn title_of(page: &ProfilePage, state: &AppState) -> String {
    match page {
        ProfilePage::User { handle, .. } => state
            .person_by_handle(handle)
            .map(|p| p.display_name())
            .unwrap_or_else(|| format!("@{handle}")),
        ProfilePage::Group { space_id }
        | ProfilePage::Agent {
            project_id: space_id,
        } => state
            .projects
            .iter()
            .find(|p| &p.id == space_id)
            .map(|p| p.name.clone())
            .unwrap_or_default(),
        ProfilePage::AgentLog { .. } => "Session log".to_string(),
        ProfilePage::Me => "You".to_string(),
        ProfilePage::Server { server_id } => state
            .servers
            .iter()
            .find(|s| &s.id == server_id)
            .map(|s| s.name.clone())
            .unwrap_or_default(),
        ProfilePage::Channel {
            server_id,
            channel_id,
        } => state
            .server(server_id)
            .and_then(|s| s.channel(channel_id))
            .map(|c| c.name.clone())
            .unwrap_or_default(),
    }
}

/// Is this page the one open in the right pane (the header pill / server
/// header ring)? Same subject counts: a person's page opened from a chip is
/// the DM pill's person too (`ProfilePage::same_subject`).
pub fn is_open(state: &AppState, page: &ProfilePage) -> bool {
    state.profile_shows(page)
}

fn glyph_tile(glyph: &'static str, theme: &Theme) -> AnyElement {
    div()
        .size(px(HERO_AVATAR))
        .rounded(px(HERO_AVATAR * 0.28))
        .bg(theme.control_fill())
        .flex()
        .items_center()
        .justify_center()
        .child(sf_symbols::glyph(glyph, HERO_AVATAR * 0.45, theme.text))
        .into_any_element()
}

/// Child index of row `open` in a kit card (one child per row — kit rows
/// carry their own hairlines, so the card does not interleave separators).
fn child_index_of_row(rows: usize, open: usize) -> Option<usize> {
    (open < rows).then_some(open)
}

/// A pane card of kit rows: one fill, no extra separators (each `kit::row`
/// hangs its own hairline; stacking `kit::section` on top doubled the line).
fn kit_card(rows: Vec<AnyElement>, theme: &Theme) -> Div {
    kit::card(theme).children(rows)
}

fn section(part: Section, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    match part {
        Section::Title(t) => kit::section_title(t, theme).into_any_element(),
        Section::Card(rows) => kit_card(rows, theme).into_any_element(),
        Section::Reveal { rows, open } => {
            let child = child_index_of_row(rows.len(), open);
            let card = kit_card(rows, theme);
            match child {
                Some(ix) => {
                    let reveal =
                        cx.listener(move |this, bounds: &Vec<Bounds<Pixels>>, window, cx| {
                            this.reveal_pane_row(bounds.get(ix), window, cx);
                        });
                    card.on_children_prepainted(move |bounds, window, cx| {
                        reveal(&bounds, window, cx)
                    })
                    .into_any_element()
                }
                None => card.into_any_element(),
            }
        }
        Section::Picker {
            header,
            swatches,
            note,
        } => kit::card(theme)
            .child(header)
            .child(div().pt(px(14.0)).child(swatches))
            .child(
                div()
                    .px(px(kit::INSET))
                    .pb(px(12.0))
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.label_muted())
                    .child(note),
            )
            .into_any_element(),
        Section::Empty(t) => kit::empty(&t, theme).into_any_element(),
        Section::Footnote(t) => kit::footnote(t, theme).into_any_element(),
        Section::Custom(el) => el,
    }
}

/// The blocks of a pane, top to bottom — the structure every kind shares
/// (Jacob, 2026-10-22 22:32 UTC: same structure, not the same tab count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Hero,
    /// Only when the kind has more than one tab.
    TabPill,
    /// The selected tab's sections.
    Options,
    /// Pinned after the options (Block user, Delete agent, Sign out…).
    Footer,
}

/// What [`render`] lays out for `tabs` tabs with `footer_sections` pinned
/// parts: hero first, then the pill (2+ tabs), then the options, then the
/// footer when there is one.
pub fn layout(tabs: usize, footer_sections: usize) -> Vec<Block> {
    let mut out = vec![Block::Hero];
    if tabs > 1 {
        out.push(Block::TabPill);
    }
    out.push(Block::Options);
    if footer_sections > 0 {
        out.push(Block::Footer);
    }
    out
}

/// Hero + pill + sections inside the frame (×, ‹ when the stack is non-empty,
/// Done, the shared scroll handle, the opaque top bar).
pub fn render(pane: ProfilePane, shell: &Shell, cx: &mut Context<Shell>) -> Stateful<Div> {
    let theme = Theme::of(cx).clone();
    let title = title_of(&pane.page, shell.state.read(cx));
    let picture = match pane.hero.picture {
        Picture::Element(el) => el,
        Picture::Glyph(glyph) => glyph_tile(glyph, &theme),
    };
    let name_el = match pane.hero.badge {
        Some(glyph) => div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(pane.hero.name)
            .child(sf_symbols::glyph(glyph, 14.0, theme.label_muted()))
            .into_any_element(),
        None => pane.hero.name,
    };
    let subtitle = pane.hero.subtitle.unwrap_or_default();
    // Hero + tab pill pin under the top bar; the sections scroll beneath them
    // and fade out at the join (Jacob, Oct 22: sticky header, no hard cut).
    let mut pinned = div().flex().flex_col().child(
        hero(
            picture,
            name_el,
            &subtitle,
            pane.hero.subtitle_color,
            pane.hero.subtitle_click,
            pane.hero.subtitle_element,
            &theme,
        )
        .children(pane.hero.editor)
        .children(
            pane.hero
                .action
                .map(|a| div().pt(px(HERO_ACTION_GAP)).child(a)),
        ),
    );
    let blocks = layout(pane.tabs.len(), pane.footer.len());
    if blocks.contains(&Block::TabPill) {
        pinned = pinned.child(tab_bar(pane.tabs, pane.selected, &theme, cx));
    }
    // The body starts under the pinned block's bottom margin (pane_frame pulls
    // it up by PANE_FADE_BAND); this spacer puts the first card back where
    // the margin had it, so only scrolled rows cross the fade. A section label
    // first ("Coding") pulls the spacer in by its own top padding, so the
    // label's text lands where a card's top edge would — Grok's 24 under the
    // pill ([feedback info-pane-chrome-2026-10-24]).
    let spacer = first_spacer(pane.body.first());
    let mut body = div()
        .flex()
        .flex_col()
        .pb(px(16.0))
        .child(div().h(px(spacer)).flex_shrink_0());
    for part in pane.body {
        body = body.child(section(part, &theme, cx));
    }
    if blocks.contains(&Block::Footer) {
        body = body.child(div().h(px(14.0)));
        for part in pane.footer {
            body = body.child(section(part, &theme, cx));
        }
    }
    let share = pane.share.filter(|_| share_disc_allowed(&pane.page));
    pushed_frame(
        &title,
        Some(pinned),
        body,
        pane.done,
        share,
        Some(&shell.profile_scroll),
        shell,
        &theme,
        cx,
    )
}

/// A leading section label starts this much higher than a card would, so its
/// glyphs (about 3 pt into the label's box, over `kit::SECTION_TITLE_TOP`)
/// land Grok's 24 pt under the pill: 18 − 7 + 10 + 3 = 24
/// (`grok-right-pane-details.png`: pill bottom 244, "Coding" top 268).
pub(crate) const FIRST_TITLE_PULL: f32 = 7.0;

/// Extra room over the hero's action pill: with the hero column's 6 pt gap the
/// pill sits 12 under the "@handle" line, a control's gap and a half.
pub(crate) const HERO_ACTION_GAP: f32 = 6.0;

/// Which panes may carry Grok's share disc beside the ×: the agent pane (its
/// owner's invite sheet) and no other. Jacob 2026-10-25: "remove this button
/// from side bar on group/" — groups and servers draw the × alone, like the
/// person, channel and own-profile panes ([feedback info-pane-no-share-disc-groups-2026-10-25]).
pub(crate) fn share_disc_allowed(page: &ProfilePage) -> bool {
    match page {
        ProfilePage::Agent { .. } => true,
        ProfilePage::Group { .. }
        | ProfilePage::Server { .. }
        | ProfilePage::Channel { .. }
        | ProfilePage::User { .. }
        | ProfilePage::AgentLog { .. }
        | ProfilePage::Me => false,
    }
}

/// The spacer over the first section: the fade band, pulled in when a section
/// label leads (see [`FIRST_TITLE_PULL`]).
pub(crate) fn first_spacer(first: Option<&Section>) -> f32 {
    match first {
        Some(Section::Title(_)) => PANE_FADE_BAND - FIRST_TITLE_PULL,
        _ => PANE_FADE_BAND,
    }
}

#[cfg(test)]
mod tests {
    use super::{child_index_of_row, layout, share_disc_allowed, Block};

    /// Every kind's tab set (the same labels and order as the iPhone page of
    /// that kind — `ArbosTests/ProfilePaneStructureTests.swift` holds the twin).
    /// Kinds may differ in tab count (Jacob, 2026-10-22 22:32 UTC); what every
    /// kind shares is the structure: hero, then the pill, then the options.
    const KINDS: [(&str, &[&str]); 6] = [
        ("person", &super::super::user::TABS),
        ("own profile", &super::super::me::TABS),
        ("agent", &super::super::agent::TABS),
        ("group", &super::super::group::TABS),
        ("server", &super::super::server::TABS),
        ("channel", &super::super::channel::TABS),
    ];

    #[test]
    fn every_kind_lays_out_hero_then_pill_then_options() {
        for (kind, tabs) in KINDS {
            assert!(tabs.len() >= 2, "{kind}: a pane has at least two tabs");
            let blocks = layout(tabs.len(), 1);
            assert_eq!(
                blocks,
                vec![Block::Hero, Block::TabPill, Block::Options, Block::Footer],
                "{kind}"
            );
            let no_footer = layout(tabs.len(), 0);
            assert_eq!(
                no_footer,
                vec![Block::Hero, Block::TabPill, Block::Options],
                "{kind}"
            );
        }
        // One tab: no pill, the options follow the hero directly.
        assert_eq!(layout(1, 0), vec![Block::Hero, Block::Options]);
    }

    #[test]
    fn kinds_keep_their_own_tab_sets() {
        // Grok Bot's agent tabs (Jacob 2026-10-24, [feedback agent-pane-details-tab-2026-10-24]).
        let agent: Vec<&str> = super::super::agent::TABS.to_vec();
        assert_eq!(agent, ["Details", "Library", "Computer"]);
        assert_eq!(agent[super::super::agent::TAB_LIBRARY], "Library");
        assert_eq!(agent[super::super::agent::TAB_COMPUTER], "Computer");
        assert_eq!(super::super::me::TABS, ["Profile", "Settings"]);
        assert_eq!(super::super::user::TABS, ["Links", "Groups"]);
        assert_eq!(super::super::group::TABS, ["Members", "Library"]);
        assert_eq!(
            super::super::server::TABS,
            ["Channels", "Members", "Settings"]
        );
        assert_eq!(super::super::channel::TABS, ["Profile", "Settings"]);
        for (kind, tabs) in KINDS {
            let mut seen = std::collections::HashSet::new();
            assert!(
                tabs.iter().all(|t| seen.insert(*t)),
                "{kind}: a tab label appears once"
            );
        }
    }

    /// Which panes carry Grok's share disc beside the ×: the agent pane (its
    /// owner's invite sheet) and no other — Jacob 2026-10-25, "remove this
    /// button from side bar on group/" ([feedback info-pane-no-share-disc-groups-2026-10-25]).
    #[test]
    fn only_the_agent_pane_may_have_a_share_disc() {
        use crate::app::ProfilePage;
        let id = || "x".to_string();
        assert!(share_disc_allowed(&ProfilePage::Agent { project_id: id() }));
        assert!(!share_disc_allowed(&ProfilePage::Group { space_id: id() }));
        assert!(!share_disc_allowed(&ProfilePage::Server {
            server_id: id()
        }));
        assert!(!share_disc_allowed(&ProfilePage::Channel {
            server_id: id(),
            channel_id: id()
        }));
        assert!(!share_disc_allowed(&ProfilePage::User {
            handle: id(),
            space_id: None
        }));
        assert!(!share_disc_allowed(&ProfilePage::Me));
    }

    /// Kit cards are one child per row (hairlines hang from the rows), so
    /// row `i` is child `i`; a row past the end is no child at all.
    #[test]
    fn reveal_row_maps_to_section_child() {
        assert_eq!(child_index_of_row(2, 0), Some(0));
        assert_eq!(child_index_of_row(2, 1), Some(1));
        assert_eq!(child_index_of_row(3, 2), Some(2));
        assert_eq!(child_index_of_row(2, 2), None);
        assert_eq!(child_index_of_row(0, 0), None);
    }
}
