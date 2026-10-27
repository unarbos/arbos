//! The Library tab as Grok Bot's (Jacob, 2026-10-25, "make library list based
//! with hover pop out" — [feedback library-list-hover-mac-2026-10-25]): **one
//! list** of everything shared in the chats — images, videos, files and
//! links together, newest first — grouped by day (Today · Yesterday · the
//! weekday · "Oct 18") inside the pane's cards. A row is a 32 pt thumbnail
//! or type tile, the name on one line, and the relative time ("13h"); under
//! the pointer the time gives way to a ⋯ disc that opens the row's menu.
//! A day shows five rows and a **Show more** row that unfolds the rest in
//! place. Resting on a row for [`HOVER_DELAY_MS`] floats a **preview card**
//! 8 pt above it (below it when the row is near the top): the picture or a
//! big type tile, the type ("Image"), the full name wrapping, and who shared
//! it with the sender's picture and "Arbos · Yesterday 10:06 PM". A click on
//! the row opens the viewer (links open in the browser), as before.
//!
//! Metrics: the pane kit's (row 41, inset 16, radius 12, labels at 28); the
//! list and card sizes are the approved mock's — no Grok Library shot exists
//! to measure, so the row type is Grok's 13 pt row and the thumbnail 32 pt.
//! Mock: `media/feedback/library-list/` in the project store.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use gpui::{
    anchored, deferred, div, img, prelude::*, px, Anchor, AnyElement, Bounds, ClickEvent, Context,
    Div, Edges, ElementId, FontWeight, ObjectFit, Pixels, Point, SharedString, Stateful, Window,
};

use crate::app::{AppState, ViewerItem};
use crate::model::{now_ms, AttachmentKind, Person};
use crate::ui::avatars;
use crate::ui::glass::{self, MENU_CARD_BORDER, MENU_CARD_PADDING, MENU_GAP, MENU_ROW_HEIGHT};
use crate::ui::icons::icon;
use crate::ui::message_menu::place;
use crate::ui::people;
use crate::ui::shell::Shell;
use crate::ui::sidebar;
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};

use super::pane::Section;
use super::{kit, Content, Found, Link, Sender};

/// A row: the kit's one-line row.
pub const ROW_H: f32 = kit::ROW_MIN_H;
/// The thumbnail / type tile and its corners (mock: 32, radius 7).
pub const THUMB: f32 = 32.0;
pub const THUMB_RADIUS: f32 = 7.0;
/// Right padding of a row: the ⋯ disc sits 10 from the card's edge.
pub const ROW_PAD_RIGHT: f32 = 10.0;
/// The hover ⋯ disc.
pub const MORE_DISC: f32 = 24.0;
/// The "Show more" row.
pub const SHOW_MORE_H: f32 = 36.0;
/// A day shows this many rows before "Show more".
pub const SHOW_MORE_AFTER: usize = 5;
/// The preview card: width, left edge from the pane's edge, air to the row,
/// padding, picture height, picture corners, the sender's picture.
pub const CARD_WIDTH: f32 = 300.0;
pub const CARD_LEFT: f32 = 20.0;
pub const CARD_GAP: f32 = 8.0;
pub const CARD_PAD: f32 = 10.0;
pub const CARD_IMAGE_H: f32 = 156.0;
pub const CARD_IMAGE_RADIUS: f32 = 8.0;
pub const CARD_AVATAR: f32 = 18.0;
/// Title lines in the card before the ellipsis.
pub const CARD_TITLE_LINES: usize = 3;
/// Resting this long on a row floats its card.
pub const HOVER_DELAY_MS: u64 = 300;
/// The card flips under the row when it would reach into this band under
/// the window's top.
pub const CARD_MIN_TOP: f32 = 8.0;
/// The ⋯ menu.
pub const MENU_WIDTH: f32 = 200.0;

/// Rig: `ARBOS_LIBRARY_HOVER=<row>` draws that row hovered with its card up
/// (screenshots without a pointer).
fn rig_hover() -> Option<Hover> {
    std::env::var("ARBOS_LIBRARY_HOVER")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .map(|ix| Hover {
            ix,
            epoch: 0,
            armed: true,
        })
}

/// One thing shared: an attachment (image, video, file) or a link.
pub(crate) enum Entry {
    Attachment(Found),
    Link(Link),
}

impl Entry {
    pub fn when(&self) -> u64 {
        match self {
            Entry::Attachment(f) => f.when,
            Entry::Link(l) => l.when,
        }
    }

    /// The row's one line: the file name (its title when it has no name), the
    /// link without its scheme.
    pub fn title(&self) -> String {
        match self {
            Entry::Attachment(f) => {
                if f.attachment.name.is_empty() {
                    f.attachment.title.clone().unwrap_or_default()
                } else {
                    f.attachment.name.clone()
                }
            }
            Entry::Link(l) => bare_url(&l.url),
        }
    }

    /// Who shared it, by name: the agent's project name, a person's name, else
    /// what the row carried ("@sam", "You").
    pub fn by(&self, state: &AppState) -> String {
        let raw = match self {
            Entry::Attachment(f) => f.by.as_str(),
            Entry::Link(l) => l.by.as_str(),
        };
        match self.sender() {
            Sender::Agent(pid) => state
                .project(pid)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| raw.to_string()),
            Sender::Person(p) => p
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| raw.to_string()),
            Sender::Me => raw.to_string(),
        }
    }

    fn sender(&self) -> &Sender {
        match self {
            Entry::Attachment(f) => &f.sender,
            Entry::Link(l) => &l.sender,
        }
    }

    /// "Image" · "Video" · "PDF" (the extension) · "File" · "Link".
    pub fn kind_label(&self) -> String {
        match self {
            Entry::Attachment(f) => match f.attachment.kind() {
                AttachmentKind::Image => "Image".into(),
                AttachmentKind::Video => "Video".into(),
                _ => extension(&f.attachment.name).unwrap_or_else(|| "File".into()),
            },
            Entry::Link(_) => "Link".into(),
        }
    }
}

/// `https://arbos.life/launch` → `arbos.life/launch`.
pub fn bare_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_end_matches('/')
        .to_string()
}

/// "PDF", "ZIP", "XLSX" — the upper-cased extension when it reads as one.
pub fn extension(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_uppercase())
        .filter(|e| (2..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// "now", "42m", "13h", "2d", "3w", "4mo", "1y".
pub fn rel_time(now: u64, when: u64) -> String {
    let s = now.saturating_sub(when) / 1000;
    let m = s / 60;
    let h = m / 60;
    let d = h / 24;
    if m == 0 {
        "now".into()
    } else if h == 0 {
        format!("{m}m")
    } else if d == 0 {
        format!("{h}h")
    } else if d < 7 {
        format!("{d}d")
    } else if d < 30 {
        format!("{}w", d / 7)
    } else if d < 365 {
        format!("{}mo", d / 30)
    } else {
        format!("{}y", d / 365)
    }
}

/// The day's label — the sidebar's date words: Today, Yesterday, the weekday
/// inside a week, "Oct 18" after that ("Oct 18, 2025" in another year).
pub fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        return "Today".into();
    }
    if today.pred_opt() == Some(day) {
        return "Yesterday".into();
    }
    let days = (today - day).num_days();
    if (0..7).contains(&days) {
        return day.format("%A").to_string();
    }
    if day.year() == today.year() {
        day.format("%b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    }
}

/// Items (newest first) → days (newest first), each with its items in order.
/// An item with no date lands under "Earlier".
pub fn group_days<T>(
    items: Vec<T>,
    date_of: impl Fn(&T) -> Option<NaiveDate>,
    today: NaiveDate,
) -> Vec<(String, Vec<T>)> {
    let mut out: Vec<(String, Vec<T>)> = Vec::new();
    for item in items {
        let label = match date_of(&item) {
            Some(day) => day_label(day, today),
            None => "Earlier".into(),
        };
        match out.last_mut() {
            Some((last, rows)) if *last == label => rows.push(item),
            _ => out.push((label, vec![item])),
        }
    }
    out
}

/// How many of a day's `count` rows show: all when unfolded or short, else
/// [`SHOW_MORE_AFTER`].
pub fn visible_rows(count: usize, expanded: bool) -> usize {
    if expanded || count <= SHOW_MORE_AFTER {
        count
    } else {
        SHOW_MORE_AFTER
    }
}

/// The card's height for a title of `title_chars`: padding, picture, the
/// type line, the title's lines (13 pt on a 17 pt line, ~7.2 pt a glyph
/// across 278 pt, at most [`CARD_TITLE_LINES`]), the sender line.
pub fn card_height(title_chars: usize) -> f32 {
    let per_line = ((CARD_WIDTH - 2.0 * CARD_PAD) / 7.2).floor().max(1.0) as usize;
    let lines = title_chars.div_ceil(per_line).clamp(1, CARD_TITLE_LINES) as f32;
    2.0 * CARD_PAD + CARD_IMAGE_H + 10.0 + 14.0 + 3.0 + lines * 17.0 + 8.0 + CARD_AVATAR + 2.0
}

/// Above the row when the card fits between the row and the window's top
/// band, else under it. `row_top` is the row's top in window points.
pub fn card_above(row_top: f32, title_chars: usize) -> bool {
    row_top - CARD_GAP - card_height(title_chars) >= CARD_MIN_TOP
}

fn local_date(ms: u64) -> Option<NaiveDate> {
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.date_naive())
}

// ----- the shell's state for the tab -----------------------------------------

/// What the ⋯ menu and the row click act on, captured when the row is drawn.
#[derive(Clone)]
pub(crate) enum Target {
    /// The attachment at `index` of the chat-order album the viewer shows.
    Attachment {
        album: Rc<Vec<ViewerItem>>,
        index: usize,
    },
    Link {
        url: String,
    },
}

/// The pointer rests on row `ix` of the pane opened at `epoch`
/// (`AppState::profile_opened`); `armed` once [`HOVER_DELAY_MS`] passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hover {
    pub ix: usize,
    pub epoch: u64,
    pub armed: bool,
}

/// The ⋯ menu open at `at` for `target`.
#[derive(Clone)]
pub(crate) struct LibraryMenu {
    pub target: Target,
    pub at: Point<Pixels>,
}

/// The tab's transient state on the shell: hover (+ its timer's serial),
/// which days are unfolded (per opened pane — `AppState::profile_opened`),
/// where the rows landed last frame (the card's above / below call), the menu.
#[derive(Default)]
pub(crate) struct LibraryUi {
    pub hover: Option<Hover>,
    serial: u64,
    expanded: (u64, HashSet<String>),
    row_tops: HashMap<usize, f32>,
    pub menu: Option<LibraryMenu>,
}

impl LibraryUi {
    fn expanded(&self, epoch: u64, label: &str) -> bool {
        self.expanded.0 == epoch && self.expanded.1.contains(label)
    }

    fn row_top(&self, ix: usize) -> Option<f32> {
        self.row_tops.get(&ix).copied()
    }
}

impl Shell {
    /// A row's hover changed. Leaving fires for the old row after the new
    /// one's enter, so a leave only clears the hover it belongs to.
    pub(crate) fn library_hover(&mut self, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            let epoch = self.state.read(cx).profile_opened;
            if self
                .library
                .hover
                .is_some_and(|h| h.ix == ix && h.epoch == epoch)
            {
                return;
            }
            self.library.serial = self.library.serial.wrapping_add(1);
            let serial = self.library.serial;
            self.library.hover = Some(Hover {
                ix,
                epoch,
                armed: false,
            });
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(HOVER_DELAY_MS))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.library.serial != serial {
                        return;
                    }
                    if let Some(h) = this.library.hover.as_mut() {
                        h.armed = true;
                        cx.notify();
                    }
                });
            })
            .detach();
            cx.notify();
        } else if self.library.hover.is_some_and(|h| h.ix == ix) {
            self.library.hover = None;
            self.library.serial = self.library.serial.wrapping_add(1);
            cx.notify();
        }
    }

    /// Show more / fewer of a day.
    pub(crate) fn library_toggle_day(&mut self, epoch: u64, label: String, cx: &mut Context<Self>) {
        if self.library.expanded.0 != epoch {
            self.library.expanded = (epoch, HashSet::new());
        }
        if !self.library.expanded.1.remove(&label) {
            self.library.expanded.1.insert(label);
        }
        cx.notify();
    }

    /// Where a card's rows landed (window points), for the card's placement.
    fn library_record_rows(&mut self, base: usize, bounds: &[Bounds<Pixels>]) {
        for (i, b) in bounds.iter().enumerate() {
            self.library
                .row_tops
                .insert(base + i, f32::from(b.origin.y));
        }
    }

    /// The ⋯ disc: its menu at the click.
    pub(crate) fn open_library_menu(
        &mut self,
        target: Target,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.close_popovers(cx);
        self.library.menu = Some(LibraryMenu { target, at });
        cx.notify();
    }

    /// Forget the hover and the menu (the pane closed, a sheet opened).
    pub(crate) fn library_reset(&mut self) {
        self.library.hover = None;
        self.library.menu = None;
        self.library.serial = self.library.serial.wrapping_add(1);
    }
}

// ----- the tab -----------------------------------------------------------------

/// The Library tab's sections: a label and a card per day.
pub(super) fn sections(
    content: Content,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let Content {
        media,
        files,
        links,
        ..
    } = content;
    if media.is_empty() && files.is_empty() && links.is_empty() {
        return vec![Section::Empty("Nothing shared here yet".into())];
    }
    // The viewer's album runs in chat order over every attachment; a row
    // opens the viewer at its own place in it (media-viewer-2026-10-23).
    let mut attachments: Vec<Found> = media.into_iter().chain(files).collect();
    attachments.sort_by_key(|f| f.when);
    let album: Rc<Vec<ViewerItem>> = Rc::new(
        attachments
            .iter()
            .map(|f| ViewerItem {
                attachment: f.attachment.clone(),
                by: f.by.clone(),
                when: f.when,
                chat_id: f.chat_id.clone(),
                part_id: f.part_id.clone(),
                picture_url: None,
            })
            .collect(),
    );
    let mut entries: Vec<(Entry, Target)> = attachments
        .into_iter()
        .enumerate()
        .map(|(index, f)| {
            (
                Entry::Attachment(f),
                Target::Attachment {
                    album: album.clone(),
                    index,
                },
            )
        })
        .chain(links.into_iter().map(|l| {
            let url = l.url.clone();
            (Entry::Link(l), Target::Link { url })
        }))
        .collect();
    entries.sort_by_key(|(e, _)| std::cmp::Reverse(e.when()));
    let now = now_ms();
    let today = Local::now().date_naive();
    let days = group_days(entries, |(e, _)| local_date(e.when()), today);
    let cx: &Context<Shell> = cx;
    let state = shell.state.read(cx);
    let epoch = state.profile_opened;
    // A hover from another pane (the pane changed under the pointer) is stale.
    let hovering = shell
        .library
        .hover
        .filter(|h| h.epoch == epoch)
        .or_else(rig_hover);
    let mut out = Vec::new();
    let mut base = 0usize;
    for (k, (label, rows)) in days.into_iter().enumerate() {
        let count = rows.len();
        let expanded = shell.library.expanded(epoch, &label);
        let shown = visible_rows(count, expanded);
        let mut card = kit::card(theme).debug_selector(move || format!("library-day-{k}"));
        for (i, (entry, target)) in rows.into_iter().take(shown).enumerate() {
            let ix = base + i;
            let hover = hovering.filter(|h| h.ix == ix);
            card = card.child(row(ix, entry, target, hover, now, shell, state, theme, cx));
        }
        if count > SHOW_MORE_AFTER {
            card = card.child(show_more(k, expanded, epoch, label.clone(), theme, cx));
        }
        let record = cx.listener(move |this, bounds: &Vec<Bounds<Pixels>>, _, _| {
            this.library_record_rows(base, bounds);
        });
        let card =
            card.on_children_prepainted(move |bounds, window, cx| record(&bounds, window, cx));
        out.push(Section::Title(label.into()));
        out.push(Section::Custom(card.into_any_element()));
        base += shown;
    }
    out
}

/// One row: thumbnail / type tile, the name, the time — or the ⋯ disc while
/// hovered — and, once the hover is armed, the preview card floating off it.
#[allow(clippy::too_many_arguments)]
fn row(
    ix: usize,
    entry: Entry,
    target: Target,
    hover: Option<Hover>,
    now: u64,
    shell: &Shell,
    state: &AppState,
    theme: &Theme,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let hovered = hover.is_some();
    let title = entry.title();
    let click_target = target.clone();
    let mut el = div()
        .id(ElementId::Name(SharedString::from(format!(
            "library-row-{ix}"
        ))))
        .debug_selector(move || format!("library-row-{ix}"))
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(kit::GAP))
        .h(px(ROW_H))
        .pl(px(kit::INSET))
        .pr(px(ROW_PAD_RIGHT))
        .text_size(px(TEXT_SMALL))
        .text_color(theme.text)
        .cursor_pointer()
        .when(hovered, |el| el.bg(theme.surface_2))
        .child(thumb(&entry, theme))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::NORMAL)
                .child(title.clone()),
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.library_hover(ix, *hovered, cx);
        }))
        .on_click(cx.listener(move |this, _, _, cx| {
            open(this, &click_target, cx);
        }));
    el = if hovered {
        el.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "library-more-{ix}"
                ))))
                .debug_selector(move || format!("library-more-{ix}"))
                .size(px(MORE_DISC))
                .flex_shrink_0()
                .rounded_full()
                .bg(theme.border_strong)
                .flex()
                .items_center()
                .justify_center()
                .hover(|s| s.bg(theme.text_faint))
                .child(icon("ellipsis", 14.0, theme.text))
                .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_library_menu(target.clone(), e.position(), cx);
                })),
        )
    } else {
        el.child(
            div()
                .debug_selector(move || format!("library-time-{ix}"))
                .flex_shrink_0()
                .min_w(px(MORE_DISC))
                .pr(px(6.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_muted)
                .whitespace_nowrap()
                .child(rel_time(now, entry.when())),
        )
    };
    el = el.child(kit::hairline_below(theme));
    if hover.is_some_and(|h| h.armed) {
        let above = shell
            .library
            .row_top(ix)
            .is_none_or(|top| card_above(top, title.chars().count()));
        let card = preview(&entry, state, theme);
        // The card's left is CARD_LEFT from the pane's edge; the row's own
        // left is the card inset.
        let wrap = div().absolute().left(px(CARD_LEFT - kit::INSET));
        let (wrap, anchor) = if above {
            (wrap.top(px(-CARD_GAP)), Anchor::BottomLeft)
        } else {
            (wrap.top(px(ROW_H + CARD_GAP)), Anchor::TopLeft)
        };
        el = el.child(
            wrap.child(
                deferred(
                    anchored()
                        .anchor(anchor)
                        .position_mode(gpui::AnchoredPositionMode::Local)
                        .snap_to_window_with_margin(Edges::all(px(CARD_MIN_TOP)))
                        .child(card),
                )
                .with_priority(1),
            ),
        );
    }
    el
}

/// The row's click: attachments open the viewer at their place in the
/// album, links open in the browser.
fn open(shell: &mut Shell, target: &Target, cx: &mut Context<Shell>) {
    match target {
        Target::Attachment { album, index } => {
            let (album, index) = (album.as_ref().clone(), *index);
            shell
                .state
                .update(cx, |s, cx| s.open_media_viewer(album, index, cx));
        }
        Target::Link { url } => cx.open_url(url),
    }
}

/// "Show more ⌄" / "Show less ⌃" at a day's foot.
fn show_more(
    k: usize,
    expanded: bool,
    epoch: u64,
    label: String,
    theme: &Theme,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(SharedString::from(format!(
            "library-show-more-{k}"
        ))))
        .debug_selector(move || format!("library-show-more-{k}"))
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(5.0))
        .h(px(SHOW_MORE_H))
        .px(px(kit::INSET))
        .text_size(px(TEXT_SMALL))
        .text_color(theme.text_muted)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(if expanded { "Show less" } else { "Show more" })
        .child(icon(
            if expanded {
                "chevron-up"
            } else {
                "chevron-down"
            },
            11.0,
            theme.text_muted,
        ))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.library_toggle_day(epoch, label.clone(), cx);
        }))
}

/// The 32 pt leading: the picture (aspect-fill), a play glyph over a video
/// without one, the type on a tile for files, the link glyph for links.
fn thumb(entry: &Entry, theme: &Theme) -> Div {
    let tile = div()
        .size(px(THUMB))
        .flex_shrink_0()
        .rounded(px(THUMB_RADIUS))
        .overflow_hidden()
        .bg(theme.surface_2)
        .flex()
        .items_center()
        .justify_center();
    match entry {
        Entry::Attachment(f) => match (f.attachment.kind(), &f.thumbnail) {
            (AttachmentKind::Image | AttachmentKind::Video, Some(image)) => tile.child(
                img(image.clone())
                    .size(px(THUMB))
                    .rounded(px(THUMB_RADIUS))
                    .object_fit(ObjectFit::Cover),
            ),
            (AttachmentKind::Image, None) => tile.child(icon("photo", 16.0, theme.text_faint)),
            (AttachmentKind::Video, None) => tile.child(icon("video", 16.0, theme.text_faint)),
            _ => match extension(&f.attachment.name) {
                Some(e) => tile
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(e),
                None => tile.child(icon("doc", 16.0, theme.text_muted)),
            },
        },
        Entry::Link(_) => tile.child(icon("link", 15.0, theme.text_muted)),
    }
}

/// The sender's 18 pt picture for the card's last line.
fn sender_picture(sender: &Sender, state: &AppState, theme: &Theme) -> AnyElement {
    match sender {
        Sender::Person(p) => {
            people::avatar(state.avatar(&p.avatar_url), p, CARD_AVATAR, theme).into_any_element()
        }
        Sender::Agent(pid) => match state.project(pid) {
            Some(p) => avatars::project(p, CARD_AVATAR, theme).into_any_element(),
            None => avatars::element(pid, "Agent", None, CARD_AVATAR, theme).into_any_element(),
        },
        Sender::Me => {
            let me = state.account.as_ref().map(|a| Person {
                id: a.id.clone(),
                handle: a.login.clone(),
                name: Some(a.name.clone()),
                avatar_url: a.avatar_url.clone(),
                status: String::new(),
                role: String::new(),
                last_seen_at: None,
                online: false,
                avatar_color_index: None,
            });
            match me {
                Some(p) => people::avatar(state.avatar(&p.avatar_url), &p, CARD_AVATAR, theme)
                    .into_any_element(),
                None => avatars::element("me", "You", None, CARD_AVATAR, theme).into_any_element(),
            }
        }
    }
}

/// The card's fill: a step over the pane card in the dark, white in the light.
fn card_fill(theme: &Theme) -> gpui::Hsla {
    if theme.is_light() {
        theme.surface_1
    } else {
        Theme::lighter(theme.pane_card, 0.03)
    }
}

/// The floating preview: picture (or the big type tile), "Image", the full
/// name, the sender with picture and "Arbos · Yesterday 10:06 PM".
fn preview(entry: &Entry, state: &AppState, theme: &Theme) -> AnyElement {
    let picture_w = CARD_WIDTH - 2.0 * CARD_PAD;
    let frame = div()
        .w(px(picture_w))
        .h(px(CARD_IMAGE_H))
        .rounded(px(CARD_IMAGE_RADIUS))
        .overflow_hidden()
        .bg(theme.surface_2)
        .border_1()
        .border_color(theme.border)
        .flex()
        .items_center()
        .justify_center();
    let picture = match entry {
        Entry::Attachment(f) => match (f.attachment.kind(), &f.thumbnail) {
            (AttachmentKind::Image | AttachmentKind::Video, Some(image)) => frame.child(
                img(image.clone())
                    .w(px(picture_w))
                    .h(px(CARD_IMAGE_H))
                    .rounded(px(CARD_IMAGE_RADIUS))
                    .object_fit(ObjectFit::Cover),
            ),
            (AttachmentKind::Image, None) => frame.child(icon("photo", 36.0, theme.text_faint)),
            (AttachmentKind::Video, None) => frame.child(icon("video", 36.0, theme.text_faint)),
            _ => match extension(&f.attachment.name) {
                Some(e) => frame
                    .text_size(px(22.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_muted)
                    .child(e),
                None => frame.child(icon("doc", 36.0, theme.text_muted)),
            },
        },
        Entry::Link(_) => frame.child(icon("globe", 36.0, theme.text_muted)),
    };
    let when = sidebar::stamp_label(entry.when());
    glass::popover(
        div()
            .debug_selector(|| "library-preview".into())
            .w(px(CARD_WIDTH))
            .p(px(CARD_PAD))
            .rounded(px(kit::CARD_RADIUS))
            .bg(card_fill(theme))
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            .flex()
            .flex_col()
            .child(picture)
            .child(
                div()
                    .mt(px(10.0))
                    .text_size(px(TEXT_TINY))
                    .line_height(px(14.0))
                    .text_color(theme.text_muted)
                    .child(entry.kind_label()),
            )
            .child(
                div()
                    .mt(px(3.0))
                    .text_size(px(TEXT_SMALL))
                    .line_height(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .line_clamp(CARD_TITLE_LINES)
                    .child(entry.title()),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .text_size(px(TEXT_TINY))
                    .line_height(px(14.0))
                    .text_color(theme.text_muted)
                    .child(sender_picture(entry.sender(), state, theme))
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .truncate()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(entry.by(state)),
                            )
                            .child(format!(" · {when}")),
                    ),
            ),
    )
    .into_any_element()
}

// ----- the ⋯ menu -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuItem {
    Open,
    Forward,
    Download,
    OpenLink,
    CopyLink,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Open => "Open",
            MenuItem::Forward => "Forward",
            MenuItem::Download => "Download",
            MenuItem::OpenLink => "Open link",
            MenuItem::CopyLink => "Copy link",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            MenuItem::Open => "photo",
            MenuItem::Forward => "forward",
            MenuItem::Download => "arrow-down",
            MenuItem::OpenLink => "globe",
            MenuItem::CopyLink => "link",
        }
    }
}

/// What a row's ⋯ offers: attachments open, forward, download; links open
/// and copy. Pure, for the test.
pub(crate) fn menu_items(target: &Target) -> Vec<MenuItem> {
    match target {
        Target::Attachment { .. } => vec![MenuItem::Open, MenuItem::Forward, MenuItem::Download],
        Target::Link { .. } => vec![MenuItem::OpenLink, MenuItem::CopyLink],
    }
}

fn menu_size(items: usize) -> gpui::Size<Pixels> {
    let rows = items as f32;
    let h = 2.0 * (MENU_CARD_BORDER + MENU_CARD_PADDING)
        + rows * MENU_ROW_HEIGHT
        + (rows - 1.0).max(0.0) * MENU_GAP;
    gpui::size(px(MENU_WIDTH), px(h))
}

/// The open ⋯ menu, appended to the shell's root like the other menus.
pub(crate) fn menu(shell: &Shell, window: &Window, cx: &mut Context<Shell>) -> Option<Div> {
    let menu = shell.library.menu.clone()?;
    let theme = Theme::of(cx).clone();
    let items = menu_items(&menu.target);
    let origin = place(menu.at, menu_size(items.len()), window.viewport_size());
    let mut body = div().flex().flex_col().gap(px(MENU_GAP));
    for item in items {
        let target = menu.target.clone();
        body = body.child(
            glass::menu_row(
                format!(
                    "library-menu-{}",
                    item.label().to_ascii_lowercase().replace(' ', "-")
                ),
                item.glyph(),
                item.label(),
                false,
                &theme,
            )
            .debug_selector(move || {
                format!(
                    "library-menu-{}",
                    item.label().to_ascii_lowercase().replace(' ', "-")
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.library.menu = None;
                match (item, &target) {
                    (MenuItem::Open, _) => open(this, &target, cx),
                    (MenuItem::Forward, Target::Attachment { .. }) => {
                        open(this, &target, cx);
                        this.state
                            .update(cx, |s, cx| s.media_viewer_toggle_forward(cx));
                    }
                    (MenuItem::Download, Target::Attachment { album, index }) => {
                        if let Some(item) = album.get(*index) {
                            if let Some(key) = item.attachment.blob_ref.clone() {
                                let name = item.attachment.name.clone();
                                this.state
                                    .update(cx, |s, cx| s.download_attachment(key, name, cx));
                            }
                        }
                    }
                    (MenuItem::OpenLink, _) => open(this, &target, cx),
                    (MenuItem::CopyLink, Target::Link { url }) => {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                    }
                    (MenuItem::Forward | MenuItem::Download, Target::Link { .. })
                    | (MenuItem::CopyLink, Target::Attachment { .. }) => {}
                }
                cx.notify();
            })),
        );
    }
    Some(
        div()
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(glass::frosted_menu(MENU_WIDTH, &theme, body)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("date")
    }

    #[test]
    fn relative_times_are_compact() {
        let now = 1_000_000_000_000u64;
        let min = 60_000;
        assert_eq!(rel_time(now, now - 20_000), "now");
        assert_eq!(rel_time(now, now - 42 * min), "42m");
        assert_eq!(rel_time(now, now - 13 * 60 * min), "13h");
        assert_eq!(rel_time(now, now - 2 * 24 * 60 * min), "2d");
        assert_eq!(rel_time(now, now - 20 * 24 * 60 * min), "2w");
        assert_eq!(rel_time(now, now - 100 * 24 * 60 * min), "3mo");
        assert_eq!(rel_time(now, now - 800 * 24 * 60 * min), "2y");
        assert_eq!(rel_time(now, now + 5000), "now", "a clock ahead of ours");
    }

    #[test]
    fn day_labels_are_the_sidebars_words() {
        let today = d(2026, 10, 25); // a Sunday
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(d(2026, 10, 24), today), "Yesterday");
        assert_eq!(day_label(d(2026, 10, 23), today), "Friday");
        assert_eq!(day_label(d(2026, 10, 19), today), "Monday");
        assert_eq!(day_label(d(2026, 10, 18), today), "Oct 18");
        assert_eq!(day_label(d(2025, 12, 31), today), "Dec 31, 2025");
    }

    #[test]
    fn days_group_consecutive_items_newest_first() {
        let today = d(2026, 10, 25);
        let items = vec![
            ("a", Some(d(2026, 10, 25))),
            ("b", Some(d(2026, 10, 25))),
            ("c", Some(d(2026, 10, 24))),
            ("d", Some(d(2026, 10, 18))),
            ("e", None),
        ];
        let days = group_days(items, |(_, day)| *day, today);
        let shape: Vec<(String, Vec<&str>)> = days
            .into_iter()
            .map(|(l, rows)| (l, rows.into_iter().map(|(n, _)| n).collect()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("Today".to_string(), vec!["a", "b"]),
                ("Yesterday".to_string(), vec!["c"]),
                ("Oct 18".to_string(), vec!["d"]),
                ("Earlier".to_string(), vec!["e"]),
            ]
        );
    }

    #[test]
    fn five_rows_then_show_more() {
        assert_eq!(visible_rows(3, false), 3);
        assert_eq!(visible_rows(5, false), 5);
        assert_eq!(visible_rows(6, false), 5);
        assert_eq!(visible_rows(6, true), 6);
        assert_eq!(SHOW_MORE_AFTER, 5);
    }

    #[test]
    fn the_card_floats_above_unless_it_would_hit_the_top() {
        // One title line: 10 + 156 + 10 + 14 + 3 + 17 + 8 + 18 + 2 + 10 = 248.
        assert_eq!(card_height(10), 248.0);
        assert_eq!(card_height(60), 248.0 + 17.0, "two lines");
        assert_eq!(card_height(500), 248.0 + 34.0, "clamped at three");
        assert!(card_above(600.0, 10));
        assert!(!card_above(200.0, 10), "no room over a row near the top");
        assert!(card_above(248.0 + CARD_GAP + CARD_MIN_TOP, 10));
        assert!(!card_above(248.0 + CARD_GAP + CARD_MIN_TOP - 1.0, 10));
    }

    #[test]
    fn titles_and_kinds() {
        assert_eq!(
            bare_url("https://www.arbos.life/launch/"),
            "arbos.life/launch"
        );
        assert_eq!(extension("press-kit-v3.pdf").as_deref(), Some("PDF"));
        assert_eq!(extension("archive.tar.gz").as_deref(), Some("GZ"));
        assert_eq!(extension("Makefile"), None);
        assert_eq!(extension("odd.toolongext"), None);
    }

    #[test]
    fn the_menu_offers_what_the_row_can_do() {
        let link = Target::Link {
            url: "https://arbos.life".into(),
        };
        assert_eq!(menu_items(&link), [MenuItem::OpenLink, MenuItem::CopyLink]);
        let file = Target::Attachment {
            album: Rc::new(Vec::new()),
            index: 0,
        };
        assert_eq!(
            menu_items(&file),
            [MenuItem::Open, MenuItem::Forward, MenuItem::Download]
        );
    }

    #[test]
    fn the_mocks_numbers() {
        assert_eq!(ROW_H, 41.0);
        assert_eq!(THUMB, 32.0);
        assert_eq!(CARD_WIDTH, 300.0);
        assert_eq!(CARD_LEFT, 20.0);
        assert_eq!(CARD_GAP, 8.0);
        assert_eq!(HOVER_DELAY_MS, 300);
    }
}

#[cfg(test)]
mod render_tests {
    //! The real `Shell` over a real `AppState`, headless (the tasks drawer's
    //! mount): a people group whose chat holds pictures, a file and a link.
    //! The Library tab lists them in one list by day, five then "Show more",
    //! the ⋯ in the time's place under the pointer, and the preview card
    //! floating 8 pt over the row once the hover has rested.
    use std::time::Duration;

    use futures::future::BoxFuture;
    use gpui::{point, size, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::ProfilePage;
    use crate::model::{Attachment, Chat, Message, Part, Person, Project, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn shared(id: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": "c1", "role": "user", "sender": "user:gh_2",
            "senderHandle": "samir", "senderName": "Samir Haddad",
            "status": "done", "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn attachment(name: &str, mime: &str, kind: AttachmentKind) -> Part {
        Part::Attachment {
            id: format!("part-{name}"),
            attachment: Attachment {
                name: name.into(),
                mime: mime.into(),
                blob_ref: Some(format!("seed-{name}")),
                bytes: Some(212_000),
                kind: Some(kind),
                title: None,
                chart_part: None,
            },
        }
    }

    /// Seven things today (six pictures/files + a link) and one yesterday.
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_reduce_motion(true);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = now_ms();
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "g1".into(),
                name: "Launch week".into(),
                kind: SpaceKind::Group,
                people: vec![Person {
                    id: "gh_2".into(),
                    handle: "samir".into(),
                    name: Some("Samir Haddad".into()),
                    avatar_url: String::new(),
                    status: "member".into(),
                    role: "member".into(),
                    last_seen_at: None,
                    online: false,
                    avatar_color_index: None,
                }],
                ..Project::default()
            });
            let chat: Chat = serde_json::from_value(serde_json::json!({
                "id": "c1", "project_id": "g1", "title": "Launch week", "is_pinned": true,
                "is_system": false, "unread": false, "last_message_at": now,
            }))
            .expect("chat");
            s.chats.insert("g1".into(), vec![chat]);
            // Clock-independent: today's rows sit at `now` itself (a row "an hour
            // ago" was yesterday for the first hour after midnight and reddened
            // the gate), yesterday's an hour before today's local midnight.
            let today_start = Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .and_then(|t| t.and_local_timezone(Local).single())
                .map(|t| t.timestamp_millis() as u64)
                .unwrap_or(now);
            let mut today = shared("m1", "Deck, dashboard and the press kit.", now - 2);
            today.parts = vec![
                attachment("deck-v3-slide-04.png", "image/png", AttachmentKind::Image),
                attachment("press-kit-v3.pdf", "application/pdf", AttachmentKind::Pdf),
                attachment("launch-dashboard.png", "image/png", AttachmentKind::Image),
                attachment("launch-teaser.mp4", "video/mp4", AttachmentKind::Video),
                attachment("signup-funnel.png", "image/png", AttachmentKind::Image),
                attachment("launch-day-team.jpg", "image/jpeg", AttachmentKind::Image),
            ];
            let link = shared("m2", "Landing page: https://arbos.life/launch", now - 1);
            let mut yesterday = shared("m0", "", today_start.saturating_sub(3_600_000));
            yesterday.parts = vec![attachment(
                "q3-review.pdf",
                "application/pdf",
                AttachmentKind::Pdf,
            )];
            s.messages.insert("c1".into(), vec![yesterday, today, link]);
            s.selected_project = Some("g1".into());
            s.active_tab.insert("g1".into(), "c1".into());
            s.open_profile(
                ProfilePage::Group {
                    space_id: "g1".into(),
                },
                cx,
            );
            s.set_profile_tab(1, cx);
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.simulate_resize(size(px(1180.0), px(780.0)));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    #[gpui::test]
    fn the_tab_is_one_list_by_day_five_rows_then_show_more(cx: &mut TestAppContext) {
        let (_rt, _state, shell, vcx) = mount(cx);
        settle(&shell, vcx);
        // Today: the link (newest) and six attachments → five rows + Show more;
        // yesterday: one row, no Show more.
        let today = vcx.debug_bounds("library-day-0").expect("today's card");
        let yesterday = vcx.debug_bounds("library-day-1").expect("yesterday's card");
        assert!(yesterday.top() > today.bottom(), "days run newest first");
        for ix in 0..5 {
            let row = vcx
                .debug_bounds(Box::leak(format!("library-row-{ix}").into_boxed_str()))
                .unwrap_or_else(|| panic!("row {ix}"));
            assert_eq!(
                f32::from(row.size.height),
                ROW_H,
                "row {ix} is the kit's one-line row"
            );
            assert!(row.top() >= today.top() && row.bottom() <= today.bottom() + px(1.0));
        }
        assert!(
            vcx.debug_bounds("library-row-5").is_some(),
            "yesterday's one row follows today's five"
        );
        assert!(
            vcx.debug_bounds("library-row-6").is_none(),
            "two of today's rows are folded"
        );
        let more = vcx
            .debug_bounds("library-show-more-0")
            .expect("Show more under today");
        assert_eq!(f32::from(more.size.height), SHOW_MORE_H);
        assert!(
            vcx.debug_bounds("library-show-more-1").is_none(),
            "a day with one row has no Show more"
        );
        // At rest every row shows its time, no ⋯, no card.
        assert!(vcx.debug_bounds("library-time-0").is_some());
        assert!(vcx.debug_bounds("library-more-0").is_none());
        assert!(vcx.debug_bounds("library-preview").is_none());

        // Show more unfolds the day in place.
        vcx.simulate_click(centre(more), Modifiers::default());
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("library-row-6").is_some(), "the sixth row");
        assert!(
            vcx.debug_bounds("library-row-7").is_some(),
            "yesterday's row, re-indexed"
        );
        assert!(vcx.debug_bounds("library-row-8").is_none());
        let less = vcx
            .debug_bounds("library-show-more-0")
            .expect("Show less stays");
        vcx.simulate_click(centre(less), Modifiers::default());
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("library-row-6").is_none(), "folded again");
    }

    #[gpui::test]
    fn hover_swaps_the_time_for_the_dots_then_floats_the_card(cx: &mut TestAppContext) {
        let (_rt, _state, shell, vcx) = mount(cx);
        settle(&shell, vcx);
        // Row 5 is yesterday's file, low enough for the card to fit above it.
        let row = vcx.debug_bounds("library-row-5").expect("row");
        vcx.simulate_mouse_move(centre(row), None, Modifiers::default());
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds("library-more-5").is_some(),
            "⋯ in the time's place"
        );
        assert!(
            vcx.debug_bounds("library-time-5").is_none(),
            "the time gave way"
        );
        assert!(
            vcx.debug_bounds("library-preview").is_none(),
            "no card before the hover has rested"
        );
        let after = vcx.debug_bounds("library-row-5").expect("row");
        assert_eq!(after, row, "no layout shift under the pointer");

        vcx.executor()
            .advance_clock(Duration::from_millis(HOVER_DELAY_MS + 50));
        vcx.run_until_parked();
        settle(&shell, vcx);
        let card = vcx
            .debug_bounds("library-preview")
            .expect("the card after the delay");
        let pane_left = f32::from(row.left()) - kit::INSET;
        assert_eq!(f32::from(card.size.width), CARD_WIDTH);
        assert!(
            (f32::from(card.left()) - (pane_left + CARD_LEFT)).abs() < 1.0,
            "card left {:?} vs pane {pane_left} + {CARD_LEFT}",
            card.left()
        );
        assert!(
            (f32::from(row.top()) - f32::from(card.bottom()) - CARD_GAP).abs() < 1.0,
            "the card floats {CARD_GAP} over the row: card {card:?} row {row:?}"
        );

        // Leaving the row takes the card and the ⋯ with it.
        vcx.simulate_mouse_move(point(px(10.0), px(10.0)), None, Modifiers::default());
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("library-preview").is_none());
        assert!(vcx.debug_bounds("library-time-5").is_some());
    }

    #[gpui::test]
    fn the_dots_open_the_rows_menu(cx: &mut TestAppContext) {
        let (_rt, _state, shell, vcx) = mount(cx);
        settle(&shell, vcx);
        // Row 0 is the link (newest): Open link · Copy link.
        let row = vcx.debug_bounds("library-row-0").expect("row");
        vcx.simulate_mouse_move(centre(row), None, Modifiers::default());
        settle(&shell, vcx);
        let dots = vcx.debug_bounds("library-more-0").expect("⋯");
        assert_eq!(f32::from(dots.size.width), MORE_DISC);
        vcx.simulate_click(centre(dots), Modifiers::default());
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("library-menu-open-link").is_some());
        assert!(vcx.debug_bounds("library-menu-copy-link").is_some());
        assert!(vcx.debug_bounds("library-menu-forward").is_none());
        let open = shell.read_with(&*vcx, |sh, _| sh.library.menu.is_some());
        assert!(open);
        // A click elsewhere closes it.
        vcx.simulate_click(point(px(600.0), px(400.0)), Modifiers::default());
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("library-menu-open-link").is_none());

        // A file row: Open · Forward · Download.
        let row = vcx.debug_bounds("library-row-2").expect("row");
        vcx.simulate_mouse_move(centre(row), None, Modifiers::default());
        settle(&shell, vcx);
        let dots = vcx.debug_bounds("library-more-2").expect("⋯");
        vcx.simulate_click(centre(dots), Modifiers::default());
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("library-menu-open").is_some());
        assert!(vcx.debug_bounds("library-menu-forward").is_some());
        assert!(vcx.debug_bounds("library-menu-download").is_some());
    }
}
