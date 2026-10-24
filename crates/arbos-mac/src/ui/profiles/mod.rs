//! Profile pages in the right pane (three-pane rule, 2026-10-20): a user, a
//! group (people chat or shared project) or an agent. One pane beside the
//! chat — it never covers it — closed by the X at its top right, the same
//! skeleton for all three: hero (picture, name), tabs, then lists or
//! settings. Pictures and names are editable only on what you own.
//!
//! Everything shown is derived from state the app already holds: members from
//! the space, media / files / voice / links from the loaded chat docs,
//! presence from the room frames.

mod agent;
mod channel;
mod group;
pub(crate) mod kit;
mod me;
pub(crate) mod pane;
mod server;
pub(crate) mod user;

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Div, ElementId, FontWeight, Hsla, RenderImage,
    ScrollHandle, SharedString, Stateful,
};

use crate::app::{AppState, ProfilePage, ViewerItem};
use crate::model::{now_ms, Attachment, AttachmentKind, Chat, Id, Message, Part};
use crate::ui::glass;
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, CONTROL_GAP, CONTROL_GLYPH, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// The right pane's width (mockups b, d, g: a quarter of the window).
pub const SHEET_WIDTH: f32 = 340.0;
/// Mocks b/d/g: the picture is about a fifth of the pane's width.
pub(super) const HERO_AVATAR: f32 = 72.0;
/// The share / close discs' inset from the pane's right edge — the sidebar
/// strip's `STRIP_INSET`: Grok's close disc ends 24 px = 12 pt before the
/// window edge (`grok-right-pane-details.png`).
pub(crate) const PANE_DISC_INSET: f32 = crate::ui::sidebar::STRIP_INSET;
/// Top bar → hero picture: Grok's avatar starts 73.5 pt under the window edge
/// (rows 147–264 at 2 px/pt), 13.5 under its 60 pt band → 14.
pub(crate) const HERO_TOP: f32 = 14.0;

/// The open page, if any — rendered as the right pane.
pub fn pane(page: &ProfilePage, shell: &Shell, cx: &mut Context<Shell>) -> Option<Stateful<Div>> {
    match page {
        ProfilePage::User { handle, space_id } => {
            user::page(handle, space_id.as_deref(), shell, cx)
        }
        ProfilePage::Group { space_id } => group::page(space_id, shell, cx),
        ProfilePage::Agent { project_id } => agent::page(project_id, shell, cx),
        ProfilePage::Me => Some(me::page(shell, cx)),
        ProfilePage::Server { server_id } => server::page(server_id, shell, cx),
        ProfilePage::Channel {
            server_id,
            channel_id,
        } => channel::page(server_id, channel_id, shell, cx),
    }
}

// ----- scaffold ----------------------------------------------------------------

/// A page pushed from another (a sender tapped in a group): the header leads with `‹`
/// back to the page underneath and `title`.
#[allow(clippy::too_many_arguments)]
pub(super) fn pushed_frame(
    title: &str,
    pinned: Option<Div>,
    body: Div,
    right: Option<Stateful<Div>>,
    share: Option<HeroClick>,
    scroll: Option<&ScrollHandle>,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    if shell.state.read(cx).profile_stack.is_empty() {
        return pane_frame(
            SHEET_WIDTH,
            None,
            pinned,
            body,
            right,
            share,
            scroll,
            theme,
            cx,
        );
    }
    let back = glass::control_disc("profile-back", "chevron-left", CONTROL_GLYPH, theme)
        .on_click(cx.listener(|this, _, _, cx| this.back_profile(cx)));
    pane_frame(
        SHEET_WIDTH,
        Some((title, back)),
        pinned,
        body,
        right,
        share,
        scroll,
        theme,
        cx,
    )
}

/// The right pane's top bar: the chat header's band, Grok Bot's share and
/// close discs at its right end — [`CONTROL`] 36 pt, [`CONTROL_GAP`] 8 apart,
/// [`PANE_DISC_INSET`] 12 from the pane's edge, centred on [`CONTROL_LINE`]
/// (measured on `grok-right-pane-details.png`, 2 px/pt: discs at rows 30–101
/// = top 15 / centre 33 in a shot whose strip discs sit on the same line; in
/// `grok-chat-1.png` both sets share rows 28–99). No fill of its own: the pane
/// blends with the chat behind it ([feedback info-pane-chrome-2026-10-24]).
pub(crate) fn top_bar() -> Div {
    div()
        .h(px(crate::ui::theme::HEADER_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(CONTROL_GAP))
        .px(px(PANE_DISC_INSET))
        .debug_selector(|| "profile-top-bar".into())
}

/// The share disc of a page that has something to share (an agent, a group:
/// the invite sheet). Left of the close disc, Grok's order.
pub(crate) fn share_disc(theme: &Theme, on_click: HeroClick) -> Stateful<Div> {
    glass::control_disc("profile-share", "share", glass::PANE_SHARE_GLYPH, theme)
        .debug_selector(|| "profile-share".into())
        .on_click(move |e, w, cx| on_click(e, w, cx))
}

/// The close disc: `×` at [`glass::PANE_CLOSE_GLYPH`].
pub(crate) fn close_disc(theme: &Theme) -> Stateful<Div> {
    glass::control_disc("profile-close", "xmark", glass::PANE_CLOSE_GLYPH, theme)
        .debug_selector(|| "profile-close".into())
}

/// The pane with a pushed page: `‹` back and the page title lead the header.
/// The band over which the scrolling body fades out under the pinned block
/// (the chat header's treatment, #1336; Jacob Oct 22: "no hard cut"). It is
/// the pinned block's bottom margin (hero `pb` / tab pill `mb`): the body
/// starts that far up, under the margin, with the same amount of padding, so
/// at rest the first card sits fully visible 18 pt under the pill and only
/// scrolled rows pass through the fade. A page with no pinned block (the
/// settings subpages: `‹ Title ×`, then cards) gets the same 18 pt as top
/// padding instead, so its first card starts where the fade ends (#1365's
/// sibling, Jacob Oct 23: "the first card is cut off under the header").
pub(crate) const PANE_FADE_BAND: f32 = 18.0;

/// The pane's scrolling body under the top bar (and the pinned block, when
/// there is one), fading out over `PANE_FADE_BAND` at its top. With a pinned
/// block the body is pulled up under the block's bottom margin (the page adds
/// the matching spacer); without one it is padded down by the band, so in
/// both cases the first card at rest starts exactly where the fade ends.
pub(crate) fn scroll_body(
    body: Div,
    has_pinned: bool,
    scroll: Option<&ScrollHandle>,
) -> crate::ui::frost::EdgeFaded {
    crate::ui::frost::edge_faded(
        PANE_FADE_BAND,
        true,
        false,
        body.id("profile-body")
            .flex_1()
            .min_h_0()
            .when(has_pinned, |el| el.mt(px(-PANE_FADE_BAND)))
            .when(!has_pinned, |el| el.pt(px(PANE_FADE_BAND)))
            .debug_selector(|| "profile-body".into())
            .overflow_y_scroll()
            .when_some(scroll, |el, h| el.track_scroll(h)),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pane_frame(
    width: f32,
    back: Option<(&str, Stateful<Div>)>,
    pinned: Option<Div>,
    body: Div,
    right: Option<Stateful<Div>>,
    share: Option<HeroClick>,
    scroll: Option<&ScrollHandle>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let close = close_disc(theme)
        .on_click(cx.listener(|this, _, window, cx| this.close_profile(window, cx)));
    let share = share.map(|f| share_disc(theme, f));
    // The top bar: no hairline under it (Jacob, Oct 22) and no fill of its
    // own — the pane is the chat's surface ([feedback info-pane-chrome-2026-10-24]).
    // `pinned` (a page's hero + tab pill, D93) sits under it and stays while
    // the body scrolls beneath, fading out over `PANE_FADE_BAND` like the chat
    // under its header.
    let mut header = top_bar();
    if let Some((title, back)) = back {
        header = header.child(back).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(TEXT_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title.to_string()),
        );
    } else {
        header = header.child(div().flex_1());
    }
    let header = header.children(right).children(share).child(close);
    let has_pinned = pinned.is_some();
    // No hairline between the pane and the chat, and the chat's own surface:
    // Grok's pane is the window (`#121212` on both sides of the join, a
    // half-point `#1F1F1F` line that reads as nothing); ours was a lighter
    // panel (`#1E1E1E` on `#141414`) behind a 1 pt `border` line.
    div()
        .id("profile-pane")
        .w(px(width))
        .flex_shrink_0()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.pane())
        .child(header)
        .children(pinned.map(|p| p.flex_shrink_0()))
        .child(scroll_body(body, has_pinned, scroll))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                // The root would refocus the composer over the pane's fields.
                cx.stop_propagation();
                this.close_popovers(cx);
            }),
        )
}

/// Big avatar, name, status line.
pub(super) fn hero(
    avatar: AnyElement,
    name: AnyElement,
    status: &str,
    status_color: Hsla,
    _theme: &Theme,
) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .pt(px(HERO_TOP))
        .pb(px(18.0))
        .child(div().size(px(HERO_AVATAR)).child(avatar))
        .child(
            div()
                .pt(px(8.0))
                .w_full()
                .max_w(px(SHEET_WIDTH - 80.0))
                .flex()
                .justify_center()
                .child(name),
        )
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(status_color)
                .child(status.to_string()),
        )
}

/// A click handler a page hands the shared hero (`cx.listener(..)`, boxed).
pub(super) type HeroClick = Box<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App)>;

/// The hero picture of something you own: hover shows a crayon over it,
/// click runs `on_click` (mockup c). `None` = read-only, just the picture.
pub(super) fn editable_picture(
    id: &'static str,
    picture: AnyElement,
    on_click: Option<HeroClick>,
) -> Stateful<Div> {
    let editable = on_click.is_some();
    div()
        .id(id)
        .group("hero-picture")
        .relative()
        .size(px(HERO_AVATAR))
        .flex_shrink_0()
        .child(picture)
        .when(editable, |el| {
            el.cursor_pointer().child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(Theme::with_alpha(gpui::black(), 0.45))
                    .opacity(0.0)
                    .group_hover("hero-picture", |s| s.opacity(1.0))
                    .child(icon("pencil", 30.0, gpui::white())),
            )
        })
        .when_some(on_click, |el, f| el.on_click(move |e, w, cx| f(e, w, cx)))
}

/// The hero name of something you own: hover shows it as a field, click
/// edits it in place (`editing` swaps in the shared name input). `None` for
/// `on_click` = read-only text.
pub(super) fn editable_name(
    id: &'static str,
    name: &str,
    editing: bool,
    input: &gpui::Entity<crate::ui::composer::ComposerInput>,
    on_click: Option<HeroClick>,
    theme: &Theme,
) -> Stateful<Div> {
    let field = div()
        .id(id)
        .h(px(36.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.0))
        .border_1()
        .text_size(px(TEXT_BODY + 6.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text);
    if editing {
        return field
            .w(px(SHEET_WIDTH - 64.0))
            .bg(theme.field)
            .border_color(theme.primary)
            .child(input.clone());
    }
    let editable = on_click.is_some();
    field
        .max_w(px(SHEET_WIDTH - 64.0))
        .border_color(gpui::transparent_black())
        .when(editable, |el| {
            el.cursor_text()
                .hover(|s| s.bg(theme.field).border_color(theme.field_border))
        })
        .child(div().truncate().child(name.to_string()))
        .when_some(on_click, |el, f| el.on_click(move |e, w, cx| f(e, w, cx)))
}

pub(super) fn card(theme: &Theme) -> Div {
    kit::card(theme)
}

/// Segmented tab bar; `tabs[i]` is selected when `i == current`.
pub(super) fn tab_bar(
    tabs: &[&'static str],
    current: usize,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    // Mocks b/g: two bare labels, the current one on a capsule, centred —
    // no track, no border; 18 pt down to the first card.
    let mut bar = div()
        .flex_shrink_0()
        .mx(px(16.0))
        .mb(px(18.0))
        .flex()
        .justify_center()
        .gap(px(4.0));
    for (i, name) in tabs.iter().enumerate() {
        let on = i == current;
        bar = bar.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "profile-tab-{name}"
                ))))
                .h(px(30.0))
                .px(px(14.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TEXT_BODY))
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if on { theme.text } else { theme.text_muted })
                .bg(if on {
                    theme.surface_2
                } else {
                    gpui::transparent_black()
                })
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .child(name.to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.set_profile_tab(i, cx));
                })),
        );
    }
    bar
}

/// Centered grey line for an empty tab.
pub(super) fn empty(text: &str, theme: &Theme) -> Div {
    kit::empty(text, theme)
}

/// A list row: 36 pt picture, title, subtitle, optional trailing element —
/// the kit's row, so members, groups and files share one height and inset.
pub(super) fn list_row(
    id: impl Into<ElementId>,
    leading: AnyElement,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    _subtitle_color: Hsla,
    trailing: Option<AnyElement>,
    theme: &Theme,
) -> Stateful<Div> {
    kit::row(
        id,
        kit::Leading::Avatar(leading),
        title,
        Some(subtitle.into()),
        None,
        trailing,
        false,
        theme,
    )
}

// ----- content from the chat docs ----------------------------------------------

/// One attachment found in a chat, with where it came from.
pub(crate) struct Found {
    pub attachment: Attachment,
    /// Who shared it ("Mira", "@sam", "You", the agent's name).
    pub by: String,
    pub when: u64,
    pub thumbnail: Option<Arc<RenderImage>>,
    /// The chat and `parts[]` id it sits in (the viewer's album entry).
    pub chat_id: Id,
    pub part_id: Id,
}

/// A link found in a message.
pub(crate) struct Link {
    pub url: String,
    pub by: String,
    pub chat: String,
    pub when: u64,
}

pub(super) struct Content {
    pub media: Vec<Found>,
    pub files: Vec<Found>,
    pub voice: Vec<Found>,
    pub links: Vec<Link>,
}

/// Everything shareable in `chats`, newest first. Only loaded docs count.
pub(super) fn collect(state: &AppState, chats: &[&Chat]) -> Content {
    let mut out = Content {
        media: Vec::new(),
        files: Vec::new(),
        voice: Vec::new(),
        links: Vec::new(),
    };
    for chat in chats {
        for m in state.messages_of(&chat.id) {
            let by = sender_label(state, m);
            for part in &m.parts {
                match part {
                    Part::Attachment { id, attachment } => {
                        let thumbnail = attachment
                            .blob_ref
                            .as_ref()
                            .and_then(|k| state.thumbnails.get(k).cloned());
                        let found = Found {
                            attachment: attachment.clone(),
                            by: sender_name(state, m),
                            when: m.created_at,
                            thumbnail,
                            chat_id: chat.id.clone(),
                            part_id: id.clone(),
                        };
                        match attachment.kind() {
                            AttachmentKind::Image | AttachmentKind::Video => out.media.push(found),
                            AttachmentKind::Audio => out.voice.push(found),
                            _ => out.files.push(found),
                        }
                    }
                    Part::Text { text, .. } => {
                        for url in urls_in(text) {
                            out.links.push(Link {
                                url,
                                by: by.clone(),
                                chat: chat.title.clone(),
                                when: m.created_at,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out.media.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.files.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.voice.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.links.sort_by_key(|a| std::cmp::Reverse(a.when));
    out.links.dedup_by(|a, b| a.url == b.url);
    out
}

/// The sender's name for a file subline ("Mira"); the handle only when there is no name.
fn sender_name(state: &AppState, m: &Message) -> String {
    if let Some(n) = m.sender_name.as_deref().filter(|n| !n.is_empty()) {
        return n.to_string();
    }
    if let Some(p) = m
        .sender_handle
        .as_deref()
        .filter(|h| !h.is_empty())
        .and_then(|h| state.person_by_handle(h))
    {
        return p.display_name();
    }
    sender_label(state, m)
}

fn sender_label(state: &AppState, m: &Message) -> String {
    if let Some(h) = m.sender_handle.as_deref().filter(|h| !h.is_empty()) {
        return format!("@{h}");
    }
    if let Some(n) = m.sender_name.as_deref().filter(|n| !n.is_empty()) {
        return n.to_string();
    }
    if state.is_mine(m) {
        return "You".into();
    }
    m.sender
        .strip_prefix("agent:")
        .or_else(|| m.sender.strip_prefix("project:"))
        .and_then(|pid| state.project(pid))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Agent".into())
}

/// `http(s)://…` tokens, trailing punctuation trimmed, in order of appearance.
pub(crate) fn urls_in(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '(' || c == ')')
        .filter(|t| t.starts_with("http://") || t.starts_with("https://"))
        .map(|t| {
            t.trim_end_matches(['.', ',', ';', ':', '!', '?', '"', '\''])
                .to_string()
        })
        .filter(|t| t.len() > 8)
        .collect()
}

/// "today", "yesterday", "3 days ago"…
pub(super) fn when_text(at: u64) -> String {
    let ago = now_ms().saturating_sub(at);
    if ago < 86_400_000 {
        "Today".into()
    } else {
        crate::app::ago_text(ago)
    }
}

fn size_text(bytes: Option<u64>) -> Option<String> {
    let b = bytes?;
    Some(if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{} KB", b / 1024)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    })
}

/// Gutter between media tiles.
const MEDIA_GAP: f32 = 3.0;

/// Media tab: three square tiles per row (aspect-fill crop), small even
/// gutters; a photo / video glyph while a thumbnail loads. The cell is
/// floored against the pane's inner width (the 1 pt left border counts) —
/// at exactly a third the last tile used to wrap and leave two per row.
pub(super) fn media_grid(items: Vec<Found>, theme: &Theme, cx: &mut Context<Shell>) -> Div {
    if items.is_empty() {
        return empty("No photos or videos yet", theme);
    }
    let cell = ((SHEET_WIDTH - 1.0 - 2.0 * kit::INSET - 2.0 * MEDIA_GAP) / 3.0).floor();
    let mut grid = div()
        .flex_shrink_0()
        .flex()
        .flex_wrap()
        .gap(px(MEDIA_GAP))
        .px(px(kit::INSET))
        .pb(px(16.0));
    // The grid is newest first; the viewer's album runs in chat order, so a
    // tile opens the viewer at its own place in that order (media-viewer-2026-10-23).
    let album: Rc<Vec<ViewerItem>> = Rc::new(
        items
            .iter()
            .rev()
            .map(|f| ViewerItem {
                attachment: f.attachment.clone(),
                by: f.by.clone(),
                when: f.when,
                chat_id: f.chat_id.clone(),
                part_id: f.part_id.clone(),
            })
            .collect(),
    );
    let n = items.len();
    for (i, f) in items.iter().enumerate() {
        let album = album.clone();
        let tile = div()
            .id(ElementId::Name(SharedString::from(format!(
                "profile-media-{i}"
            ))))
            .size(px(cell))
            .rounded(px(8.0))
            .overflow_hidden()
            .bg(theme.surface_1)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let album = album.as_ref().clone();
                this.state
                    .update(cx, |s, cx| s.open_media_viewer(album, n - 1 - i, cx));
            }));
        grid = grid.child(match &f.thumbnail {
            // The tile's overflow clip is rectangular, so the picture rounds itself.
            Some(img) => tile.child(
                gpui::img(img.clone())
                    .size(px(cell))
                    .rounded(px(8.0))
                    .object_fit(gpui::ObjectFit::Cover),
            ),
            None => tile.child(icon(
                if f.attachment.kind() == AttachmentKind::Video {
                    "video"
                } else {
                    "photo"
                },
                22.0,
                theme.text_faint,
            )),
        });
    }
    grid
}

/// Files / Voice tabs: one row per attachment.
pub(super) fn file_list(items: &[Found], glyph: &'static str, none: &str, theme: &Theme) -> Div {
    if items.is_empty() {
        return empty(none, theme);
    }
    let mut list = card(theme);
    for (i, f) in items.iter().enumerate() {
        // Redesign mock d: the file name, then "212 kB · Mira · Today" — size,
        // who shared it, when.
        let title = if f.attachment.name.is_empty() {
            f.attachment.title.clone().unwrap_or_default()
        } else {
            f.attachment.name.clone()
        };
        let mut sub = vec![f.by.clone(), when_text(f.when)];
        if let Some(size) = size_text(f.attachment.bytes) {
            sub.insert(0, size);
        }
        // The tile names the type (PDF, XLS, ZIP …) when the extension says so.
        let ext = f
            .attachment
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_uppercase())
            .filter(|e| (2..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()));
        let tile = div()
            .size(px(kit::AVATAR))
            .rounded(px(10.0))
            .bg(theme.surface_2)
            .flex()
            .items_center()
            .justify_center();
        let tile = match ext {
            Some(e) => tile
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(e),
            None => tile.child(icon(glyph, 18.0, theme.text_muted)),
        };
        list = list.child(list_row(
            ElementId::Name(SharedString::from(format!("profile-file-{i}"))),
            tile.into_any_element(),
            title,
            sub.join(" · "),
            theme.text_muted,
            None,
            theme,
        ));
    }
    list
}

/// Host for a link row (no scheme, path, or leading `www.`).
fn link_host(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_string()
}

/// The site's own favicon — fetched through the avatar pipeline (ICO/PNG/JPEG).
fn favicon_url(host: &str) -> Option<String> {
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
    {
        return None;
    }
    Some(format!("https://{host}/favicon.ico"))
}

/// Links tab: the site's favicon when we have it, else a letter tile; URL; who shared it.
pub(super) fn link_list(
    items: &[Link],
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    if items.is_empty() {
        return empty("No links shared yet", theme);
    }
    let wants: Vec<String> = items
        .iter()
        .filter_map(|l| favicon_url(&link_host(&l.url)))
        .collect();
    shell.state.update(cx, |s, _| {
        for url in wants {
            s.want_avatar(url);
        }
    });
    let state = shell.state.read(cx);
    let mut list = card(theme);
    for (i, l) in items.iter().enumerate() {
        let host = link_host(&l.url);
        let letter = host
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "#".into());
        let favicon = favicon_url(&host).and_then(|u| state.avatar(&u));
        let leading = match favicon {
            Some(img) => div()
                .size(px(kit::AVATAR))
                .rounded(px(10.0))
                .overflow_hidden()
                .bg(theme.surface_2)
                .child(
                    gpui::img(img)
                        .size(px(kit::AVATAR))
                        .rounded(px(10.0))
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .into_any_element(),
            None => div()
                .size(px(kit::AVATAR))
                .rounded(px(10.0))
                .bg(crate::ui::groups::sender_color(&host, theme))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TEXT_BODY + 2.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(letter)
                .into_any_element(),
        };
        let url = l.url.clone();
        list = list.child(
            list_row(
                ElementId::Name(SharedString::from(format!("profile-link-{i}"))),
                leading,
                host,
                format!("{} · {} · {}", l.by, l.chat, when_text(l.when)),
                theme.text_muted,
                Some(
                    div()
                        .max_w(px(150.0))
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.primary)
                        .truncate()
                        .child(l.url.clone())
                        .into_any_element(),
                ),
                theme,
            )
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
        );
    }
    list
}

/// The chats of a space, for content tabs (one per agent unless multi-chat is on).
pub(super) fn chats_of<'a>(state: &'a AppState, space_id: &str) -> Vec<&'a Chat> {
    state.visible_tabs(space_id)
}

#[cfg(test)]
mod frame_tests {
    use gpui::{div, prelude::*, px, TestAppContext};

    use super::{scroll_body, PANE_FADE_BAND};
    use crate::ui::theme::HEADER_HEIGHT;

    /// The shapes a settings subpage body takes (Updates / Notifications /
    /// About start with a card; Machine with a card too; a Library-like page
    /// with a section title), with and without a pinned hero + pill block.
    #[derive(Clone, Copy)]
    enum First {
        Card,
        Title,
    }

    struct Frame {
        pinned: bool,
        first: First,
    }

    impl gpui::Render for Frame {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let header = div()
                .h(px(HEADER_HEIGHT))
                .flex_shrink_0()
                .debug_selector(|| "profile-top-bar".into());
            let mut body = div().flex().flex_col().px(px(16.0));
            if self.pinned {
                // What `pane::render` does: the spacer that puts the first card
                // back under the pinned block's bottom margin.
                body = body.child(div().h(px(PANE_FADE_BAND)).flex_shrink_0());
            }
            if let First::Title = self.first {
                body = body.child(div().h(px(20.0)).child("Images · 4"));
            }
            body = body
                .child(
                    div()
                        .h(px(48.0))
                        .debug_selector(|| "first-card".into())
                        .child("Automatic updates"),
                )
                .child(div().h(px(900.0)));
            let mut col = div()
                .w(px(340.0))
                .h(px(600.0))
                .flex()
                .flex_col()
                .child(header);
            if self.pinned {
                col = col.child(div().h(px(140.0)).flex_shrink_0().pb(px(PANE_FADE_BAND)));
            }
            col.child(scroll_body(body, self.pinned, None))
        }
    }

    #[gpui::test]
    fn the_first_card_starts_where_the_fade_ends_with_and_without_a_pinned_block(
        cx: &mut TestAppContext,
    ) {
        for pinned in [false, true] {
            for first in [First::Card, First::Title] {
                let (_view, vcx) = cx.add_window_view(|_, _| Frame { pinned, first });
                vcx.run_until_parked();
                let bar = vcx.debug_bounds("profile-top-bar").expect("bar");
                let body = vcx.debug_bounds("profile-body").expect("body");
                let card = vcx.debug_bounds("first-card").expect("card");
                let fade_end: f32 = (body.origin.y + px(PANE_FADE_BAND)).into();
                let card_top: f32 = card.origin.y.into();
                let bar_bottom: f32 = bar.bottom().into();
                let title_h = match first {
                    First::Card => 0.0,
                    First::Title => 20.0,
                };
                assert!(
                    card_top >= fade_end - 0.5,
                    "first card top {card_top} is inside the fade (ends {fade_end}), pinned={pinned}"
                );
                assert!(
                    card_top >= bar_bottom + PANE_FADE_BAND + title_h - 0.5,
                    "first card top {card_top} vs bar bottom {bar_bottom} + {PANE_FADE_BAND}, pinned={pinned}"
                );
                if !pinned {
                    // No pinned block: the body starts flush under the bar and the
                    // card sits exactly one band below it.
                    let body_top: f32 = body.origin.y.into();
                    assert_eq!(body_top, bar_bottom, "body under the bar");
                    assert!(
                        (card_top - (bar_bottom + PANE_FADE_BAND + title_h)).abs() <= 0.5,
                        "card at bar + band ({card_top})"
                    );
                }
            }
        }
    }
}

/// Grok Bot's right-pane chrome ([feedback info-pane-chrome-2026-10-24]):
/// the numbers measured on `grok-right-pane-details.png` / `grok-chat-1.png`
/// (project store, 2 px/pt) that the pane must keep.
#[cfg(test)]
mod chrome_tests {
    use gpui::{div, prelude::*, px, TestAppContext};

    use super::pane::{first_spacer, Section, FIRST_TITLE_PULL};
    use super::{close_disc, kit, share_disc, top_bar, HERO_TOP, PANE_DISC_INSET, SHEET_WIDTH};
    use crate::ui::theme::{
        AppearancePref, Surface, Theme, CONTROL, CONTROL_GAP, CONTROL_LINE, HEADER_HEIGHT,
    };

    /// Grok, pt from the window's top / the pane's edges.
    mod grok {
        /// Share and close discs: 72 px.
        pub const DISC: f32 = 36.0;
        /// 16 px between the discs.
        pub const DISC_GAP: f32 = 8.0;
        /// The close disc ends 24 px before the window edge.
        pub const DISC_INSET: f32 = 12.0;
        /// Both discs centred on the strip discs' line (rows 28–99 in
        /// `grok-chat-1.png`, where the strip's search / + discs sit too).
        pub const DISC_TOP: f32 = 12.0;
        /// Cards: 31 / 32 px in from the pane's edges.
        pub const CARD_INSET: f32 = 16.0;
        /// Section label text 56 px from the pane's edge.
        pub const LABEL_LEFT: f32 = 28.0;
        /// The avatar starts 147 px under the window edge → 13.5 under the band.
        pub const HERO_TOP: f32 = 14.0;
    }

    fn set_theme(cx: &mut TestAppContext, pref: AppearancePref) {
        cx.update(|cx| {
            let theme = Theme::for_pref(pref, Surface::Matte, cx);
            cx.set_global(theme);
        });
    }

    #[test]
    fn the_pane_is_the_chats_surface_in_both_appearances() {
        for pref in [AppearancePref::Dark, AppearancePref::Light] {
            for surface in [Surface::Matte, Surface::Glass] {
                let theme = Theme::with(pref.resolve(gpui::WindowAppearance::Dark), surface);
                assert_eq!(
                    theme.pane(),
                    theme.chat_wash(),
                    "{pref:?} {surface:?}: the pane paints what the chat column paints"
                );
                // The cards still read on it: lighter on dark, darker on light.
                let card_l = theme.pane_card.l;
                let bg_l = theme.bg.l;
                if theme.is_light() {
                    assert!(
                        card_l < bg_l - 0.02,
                        "{pref:?}: light card {card_l} on {bg_l}"
                    );
                } else {
                    assert!(
                        card_l > bg_l + 0.04,
                        "{pref:?}: dark card {card_l} on {bg_l}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_constants_are_groks() {
        assert_eq!(CONTROL, grok::DISC);
        assert_eq!(CONTROL_GAP, grok::DISC_GAP);
        assert_eq!(PANE_DISC_INSET, grok::DISC_INSET);
        assert_eq!(CONTROL_LINE - CONTROL / 2.0, grok::DISC_TOP);
        assert_eq!(kit::INSET, grok::CARD_INSET);
        assert_eq!(kit::INSET + kit::SECTION_TITLE_INDENT, grok::LABEL_LEFT);
        assert_eq!(HERO_TOP, grok::HERO_TOP);
        assert_eq!(kit::CARD_RADIUS, 12.0, "card corners: Grok's 24 px");
        assert_eq!(kit::ROW_MIN_H, 41.0, "one-line row: Grok's 82 px");
        // A leading label's glyphs land 24 under the pill: 18 − 7 + 10 + ~3.
        assert_eq!(
            first_spacer(Some(&Section::Title("Coding".into()))),
            18.0 - FIRST_TITLE_PULL
        );
        assert_eq!(first_spacer(Some(&Section::Card(Vec::new()))), 18.0);
        assert_eq!(first_spacer(None), 18.0);
    }

    struct Bar;

    impl gpui::Render for Bar {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let share = share_disc(&theme, Box::new(|_, _, _| {}));
            div()
                .w(px(SHEET_WIDTH))
                .h(px(400.0))
                .flex()
                .flex_col()
                .child(
                    top_bar()
                        .child(div().flex_1())
                        .child(share)
                        .child(close_disc(&theme)),
                )
        }
    }

    /// The share and close discs: 36 pt each, on the strip discs' line
    /// (top 12 in the 60 band), 8 apart, the close disc 12 from the pane's
    /// right edge — Grok's pixels. Before: a 30 pt bare × at top 17, inset 10.
    #[gpui::test]
    fn the_share_and_close_discs_land_on_groks_pixels(cx: &mut TestAppContext) {
        set_theme(cx, AppearancePref::Dark);
        let (_view, vcx) = cx.add_window_view(|_, _| Bar);
        vcx.run_until_parked();
        let bar = vcx.debug_bounds("profile-top-bar").expect("bar");
        let share = vcx.debug_bounds("profile-share").expect("share");
        let close = vcx.debug_bounds("profile-close").expect("close");
        let f = |p: gpui::Pixels| -> f32 { p.into() };
        assert_eq!(f(bar.size.height), HEADER_HEIGHT);
        for (name, b) in [("share", share), ("close", close)] {
            assert_eq!(f(b.size.width), grok::DISC, "{name} width");
            assert_eq!(f(b.size.height), grok::DISC, "{name} height");
            assert_eq!(f(b.origin.y), grok::DISC_TOP, "{name} top");
        }
        assert_eq!(
            f(close.right()),
            SHEET_WIDTH - grok::DISC_INSET,
            "close right edge"
        );
        assert_eq!(
            f(close.origin.x) - f(share.right()),
            grok::DISC_GAP,
            "share → close"
        );
    }

    struct Cards;

    impl gpui::Render for Cards {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let row = |label: &str| {
                kit::row(
                    label.to_string(),
                    kit::Leading::None,
                    label.to_string(),
                    None,
                    None,
                    None,
                    true,
                    &theme,
                )
                .into_any_element()
            };
            div()
                .w(px(SHEET_WIDTH))
                .h(px(600.0))
                .flex()
                .flex_col()
                .child(
                    kit::card(&theme)
                        .debug_selector(|| "card-a".into())
                        .child(row("ArbosThree")),
                )
                .child(kit::section_title("Routines", &theme))
                .child(
                    kit::card(&theme)
                        .debug_selector(|| "card-b".into())
                        .child(row("TestFlight fix audit")),
                )
        }
    }

    /// Cards inset 16 from both edges; a one-line row 41 tall; the label's
    /// box starts 28 in and the next card starts `SECTION_TITLE_BOTTOM` under
    /// it; card → label box = `CARD_GAP`.
    #[gpui::test]
    fn cards_are_inset_and_labelled_at_groks_margins(cx: &mut TestAppContext) {
        set_theme(cx, AppearancePref::Dark);
        let (_view, vcx) = cx.add_window_view(|_, _| Cards);
        vcx.run_until_parked();
        let a = vcx.debug_bounds("card-a").expect("card a");
        let b = vcx.debug_bounds("card-b").expect("card b");
        let label = vcx.debug_bounds("section-title").expect("label");
        let f = |p: gpui::Pixels| -> f32 { p.into() };
        assert_eq!(f(a.origin.x), grok::CARD_INSET, "card left inset");
        assert_eq!(
            f(a.right()),
            SHEET_WIDTH - grok::CARD_INSET,
            "card right inset"
        );
        assert_eq!(f(a.size.height), kit::ROW_MIN_H, "one-line row height");
        assert_eq!(
            f(label.origin.y) - f(a.bottom()),
            kit::CARD_GAP,
            "card → label box"
        );
        assert_eq!(f(b.origin.y), f(label.bottom()), "label box → next card");
        // The label's text: its box is the pane width, padded to 28 on the left.
        assert_eq!(f(label.origin.x), 0.0);
        assert_eq!(f(label.size.width), SHEET_WIDTH);
        assert_eq!(
            f(label.size.height),
            kit::SECTION_TITLE_TOP + kit::SECTION_TITLE_BOTTOM + kit::SECTION_TITLE_LINE,
            "label box is its padding + one 16 pt line"
        );
    }
}
