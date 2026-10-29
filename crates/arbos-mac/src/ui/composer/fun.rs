//! Emoji · Stickers · GIFs picker (ARB-544, Jacob 2026-10-28, option B): the composer's `+`
//! opens a two-row menu — "Photo or file…" (the file prompt) and "Emoji, stickers & GIFs" —
//! and the second row opens this card above the composer, left-aligned to `+`, in the `@`
//! mention picker's card style. Mock: `media/feedback/emoji-sticker-gif/sheet-picker-mock-b.png`.
//!
//! - **Emoji** — search, Recents, one Unicode category at a time (strip at the bottom), a skin
//!   tone swatch. A pick inserts the emoji at the caret; the card stays open.
//! - **Stickers** — pack switcher (the bundled Fluent 3D starter first, then the relay's packs),
//!   a 4-column grid, the pack's licence line in the footer. A tap **sends at once**.
//! - **GIFs** — "Search KLIPY" field (the provider's required attribution), term chips, a
//!   3-column grid of previews, "Powered by KLIPY" and the privacy line in the footer. A tap
//!   sends at once. Without a relay key the tab says "GIFs are not set up yet"; over the hourly
//!   budget "GIFs are busy, try again shortly." — never a retry storm.
//!
//! Emoji glyphs are the system font (Apple Color Emoji): no art asset, no licence.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{
    div, img, prelude::*, px, AnyElement, Context, ElementId, Entity, FontWeight, ObjectFit,
    RenderImage,
};
use serde::Deserialize;

use super::fun_starter::{self, Starter};
use super::{Composer, ComposerInput};
use crate::model::{
    GifItem, GifPage, MediaPick, StickerItem, StickerPack, STICKER_KEY_PREFIX, URL_KEY_PREFIX,
};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_SMALL, TEXT_TINY};

/// Card geometry (pt) — the mock's 380 × 440.
pub const WIDTH: f32 = 380.0;
pub const HEIGHT: f32 = 440.0;
const PAD: f32 = 12.0;
const TAB_H: f32 = 26.0;
const SEARCH_H: f32 = 30.0;
const EMOJI_CELL: f32 = 36.0;
const EMOJI_GLYPH: f32 = 22.0;
const STICKER_COLS: usize = 4;
const STICKER_CELL: f32 = 84.0;
const STICKER_IMG: f32 = 72.0;
const GIF_COLS: usize = 3;
const GIF_COL_W: f32 = (WIDTH - 2.0 * PAD - 2.0 * 6.0) / GIF_COLS as f32;
/// How many relay stickers a pack page shows; "More" loads the next page.
const STICKER_PAGE: usize = 48;
/// Recent emoji kept (one row).
const RECENTS: usize = 16;
/// Typing pause before a GIF search goes to the relay (one call per pause, not per key).
const GIF_DEBOUNCE: Duration = Duration::from_millis(400);
/// The `+` menu's rows.
const MENU_W: f32 = 240.0;
const MENU_ROW_H: f32 = 32.0;
/// The chips under the GIF search (plain search terms, not provider content).
const GIF_CHIPS: &[&str] = &[
    "Trending", "happy", "lol", "yes", "no", "love", "dance", "wow", "thanks",
];
/// Fitzpatrick modifiers, in swatch order (0 = none).
const TONES: &[char] = &[
    '\u{1F3FB}',
    '\u{1F3FC}',
    '\u{1F3FD}',
    '\u{1F3FE}',
    '\u{1F3FF}',
];
const TONE_SWATCH: &[u32] = &[0xF1C27D, 0xF8D9B8, 0xE0AC69, 0xC68642, 0x8D5524, 0x4B2E1A];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunTab {
    Emoji,
    Stickers,
    Gifs,
}

impl FunTab {
    fn label(self) -> &'static str {
        match self {
            FunTab::Emoji => "Emoji",
            FunTab::Stickers => "Stickers",
            FunTab::Gifs => "GIFs",
        }
    }
}

/// One emoji of `assets/fun/emoji.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Emoji {
    /// The character(s).
    pub e: String,
    /// CLDR short name, lower-case.
    pub n: String,
    /// Category id (`smileys`, `people`, …).
    pub c: String,
    /// Takes a skin tone.
    #[serde(default)]
    pub t: u8,
}

#[derive(Deserialize)]
struct EmojiFile {
    emoji: Vec<Emoji>,
}

/// Category strip: id, label, glyph.
const CATEGORIES: &[(&str, &str, &str)] = &[
    ("smileys", "Smileys & People", "😀"),
    ("people", "People & Body", "👋"),
    ("nature", "Animals & Nature", "🐻"),
    ("food", "Food & Drink", "🍔"),
    ("travel", "Travel & Places", "🚗"),
    ("activity", "Activities", "⚽"),
    ("objects", "Objects", "💡"),
    ("symbols", "Symbols", "🔣"),
    ("flags", "Flags", "🏳️"),
];

pub fn emoji_list() -> &'static [Emoji] {
    static LIST: OnceLock<Vec<Emoji>> = OnceLock::new();
    LIST.get_or_init(|| {
        serde_json::from_str::<EmojiFile>(include_str!("../../../../../assets/fun/emoji.json"))
            .map(|f| f.emoji)
            .unwrap_or_default()
    })
}

/// `emoji` with skin tone `tone` (1–5) applied when it takes one: the modifier goes right
/// after the first scalar (a ZWJ sequence keeps the rest), replacing a FE0F there.
pub fn with_tone(emoji: &Emoji, tone: usize) -> String {
    if emoji.t == 0 || tone == 0 || tone > TONES.len() {
        return emoji.e.clone();
    }
    let modifier = TONES[tone - 1];
    let mut chars = emoji.e.chars();
    let Some(first) = chars.next() else {
        return emoji.e.clone();
    };
    let rest: String = chars.collect();
    let rest = rest.strip_prefix('\u{FE0F}').unwrap_or(&rest);
    format!("{first}{modifier}{rest}")
}

/// Emoji whose name has a word starting with `query` (lower-case), at most `limit`.
pub fn search_emoji(query: &str, limit: usize) -> Vec<&'static Emoji> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut starts = Vec::new();
    let mut words = Vec::new();
    for e in emoji_list() {
        if e.n.starts_with(&q) {
            starts.push(e);
        } else if e.n.split([' ', '-', ':']).any(|w| w.starts_with(&q)) {
            words.push(e);
        }
        if starts.len() >= limit {
            break;
        }
    }
    starts.extend(words);
    starts.truncate(limit);
    starts
}

/// Which pack the Stickers tab shows: the bundled starter, or one of the relay's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackSel {
    Starter,
    Relay(usize),
}

/// The picker's own state, owned by the composer.
pub struct FunState {
    pub open: bool,
    pub menu_open: bool,
    pub tab: FunTab,
    /// The Emoji tab's search and the GIF tab's search share one field (the placeholder flips).
    pub search: Entity<ComposerInput>,
    pub category: usize,
    pub tone: usize,
    pub recents: Vec<String>,
    pub pack: PackSel,
    pub sticker_pages: usize,
    /// Decoded starter stickers, by id.
    starter: HashMap<&'static str, Arc<RenderImage>>,
    /// Bumps on every search keystroke; the debounced task only fires for the latest.
    search_serial: u64,
    /// The GIF query the grid shows (what the last debounce sent).
    pub gif_query: String,
}

impl FunState {
    pub fn new(search: Entity<ComposerInput>) -> Self {
        Self {
            open: false,
            menu_open: false,
            tab: FunTab::Emoji,
            search,
            category: 0,
            tone: 0,
            recents: Vec::new(),
            pack: PackSel::Starter,
            sticker_pages: 1,
            starter: HashMap::new(),
            search_serial: 0,
            gif_query: String::new(),
        }
    }

    fn starter_image(&mut self, s: &'static Starter) -> Option<Arc<RenderImage>> {
        if let Some(i) = self.starter.get(s.id) {
            return Some(i.clone());
        }
        let image = Arc::new(crate::app::decode_image(s.png)?);
        self.starter.insert(s.id, image.clone());
        Some(image)
    }

    fn remember(&mut self, emoji: &str) {
        self.recents.retain(|r| r != emoji);
        self.recents.insert(0, emoji.to_owned());
        self.recents.truncate(RECENTS);
    }
}

/// The starter pack as a [`StickerPack`] for sends and the switcher.
fn starter_pack() -> StickerPack {
    StickerPack {
        id: fun_starter::PACK_ID.into(),
        name: "Fluent 3D".into(),
        publisher: "Microsoft".into(),
        license: "MIT".into(),
        license_url: "https://github.com/microsoft/fluentui-emoji/blob/main/LICENSE".into(),
        attribution: "Fluent Emoji 3D · Microsoft · MIT".into(),
        animated: false,
        page_url: "https://github.com/microsoft/fluentui-emoji".into(),
        mime: "image/png".into(),
        items: Vec::new(),
    }
}

fn sticker_key(pack: &str, id: &str) -> String {
    format!("{STICKER_KEY_PREFIX}{pack}/{id}")
}

impl Composer {
    // ----- open / close ------------------------------------------------------

    /// `+`: the two-row attach menu (option B). A second click closes it.
    pub(super) fn toggle_attach_menu(&mut self, cx: &mut Context<Self>) {
        if self.fun.open {
            self.close_fun(cx);
            return;
        }
        self.fun.menu_open = !self.fun.menu_open;
        cx.notify();
    }

    pub(super) fn open_fun(&mut self, tab: FunTab, cx: &mut Context<Self>) {
        self.fun.menu_open = false;
        self.fun.open = true;
        self.fun.tab = tab;
        self.sync_fun_search(cx);
        if tab == FunTab::Stickers {
            self.state.update(cx, |s, cx| s.ensure_sticker_packs(cx));
        }
        if tab == FunTab::Gifs {
            self.state
                .update(cx, |s, cx| s.gif_search(String::new(), cx));
        }
        cx.notify();
    }

    pub(super) fn close_fun(&mut self, cx: &mut Context<Self>) {
        self.fun.open = false;
        self.fun.menu_open = false;
        self.fun.search.update(cx, |i, cx| i.clear(cx));
        cx.notify();
    }

    /// Esc with the picker or the menu open closes it; true when there was one.
    pub(super) fn dismiss_fun(&mut self, cx: &mut Context<Self>) -> bool {
        if self.fun.open || self.fun.menu_open {
            self.close_fun(cx);
            return true;
        }
        false
    }

    fn set_fun_tab(&mut self, tab: FunTab, cx: &mut Context<Self>) {
        if self.fun.tab == tab {
            return;
        }
        self.fun.tab = tab;
        self.fun.search.update(cx, |i, cx| i.clear(cx));
        self.sync_fun_search(cx);
        match tab {
            FunTab::Stickers => self.state.update(cx, |s, cx| s.ensure_sticker_packs(cx)),
            FunTab::Gifs => {
                self.fun.gif_query.clear();
                self.state
                    .update(cx, |s, cx| s.gif_search(String::new(), cx));
            }
            FunTab::Emoji => {}
        }
        cx.notify();
    }

    fn sync_fun_search(&mut self, cx: &mut Context<Self>) {
        let placeholder = match self.fun.tab {
            FunTab::Emoji => "Search emoji",
            FunTab::Stickers => "Search stickers",
            // KLIPY's required attribution: the search placeholder.
            FunTab::Gifs => "Search KLIPY",
        };
        self.fun
            .search
            .update(cx, |i, _| i.set_placeholder(placeholder));
    }

    /// The search field changed: emoji filter at once, GIF search after a pause.
    pub(super) fn fun_search_changed(&mut self, cx: &mut Context<Self>) {
        if !self.fun.open {
            return;
        }
        if self.fun.tab != FunTab::Gifs {
            cx.notify();
            return;
        }
        self.fun.search_serial = self.fun.search_serial.wrapping_add(1);
        let serial = self.fun.search_serial;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GIF_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.fun.search_serial != serial || !this.fun.open {
                    return;
                }
                let text = this.fun.search.read(cx).text().trim().to_owned();
                this.fun.gif_query = text.clone();
                this.state.update(cx, |s, cx| s.gif_search(text, cx));
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_emoji(&mut self, emoji: String, cx: &mut Context<Self>) {
        self.fun.remember(&emoji);
        self.input
            .update(cx, |input, cx| input.insert_at_caret(&emoji, cx));
        cx.notify();
    }

    fn pick_sticker(
        &mut self,
        pack: StickerPack,
        item: StickerItem,
        bytes: Arc<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.send_media(
                MediaPick::Sticker {
                    pack: Box::new(pack),
                    item,
                    bytes,
                },
                cx,
            )
        });
        self.close_fun(cx);
    }

    fn pick_gif(&mut self, gif: GifItem, cx: &mut Context<Self>) {
        self.state
            .update(cx, |s, cx| s.send_media(MediaPick::Gif(Box::new(gif)), cx));
        self.close_fun(cx);
    }

    // ----- render ------------------------------------------------------------

    /// The `+` menu: "Photo or file…" and "Emoji, stickers & GIFs", above the `+`.
    pub(super) fn render_attach_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let row = |id: &'static str, glyph: &'static str, label: &'static str, theme: &Theme| {
            div()
                .id(id)
                .h(px(MENU_ROW_H))
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(10.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .map(|el| highlight::button(el, theme))
                .child(sf_symbols::glyph(glyph, 15.0, theme.text_muted))
                .child(
                    div()
                        .text_size(px(TEXT_SMALL))
                        .text_color(theme.text)
                        .child(label),
                )
        };
        let menu = div()
            .id("attach-menu")
            .debug_selector(|| "composer-attach-menu".into())
            .flex()
            .flex_col()
            .w(px(MENU_W))
            .p(px(6.0))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            // The menu floats over the transcript: its own hitbox must block the rows
            // beneath.
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                row("attach-menu-file", "photo", "Photo or file…", &theme).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.fun.menu_open = false;
                        this.attach(cx);
                        cx.notify();
                    },
                )),
            )
            .child(
                row(
                    "attach-menu-fun",
                    "face.smiling",
                    "Emoji, stickers & GIFs",
                    &theme,
                )
                .debug_selector(|| "composer-attach-fun".into())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open_fun(FunTab::Emoji, cx);
                })),
            );
        menu.into_any_element()
    }

    /// The picker card above the composer.
    pub(super) fn render_fun(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let tab = self.fun.tab;
        let tabs = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(3.0))
            .rounded_full()
            .bg(theme.surface_2)
            .children(
                [FunTab::Emoji, FunTab::Stickers, FunTab::Gifs]
                    .into_iter()
                    .map(|t| {
                        let on = t == tab;
                        div()
                            .id(ElementId::Name(format!("fun-tab-{}", t.label()).into()))
                            .h(px(TAB_H - 6.0))
                            .px(px(12.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .when(on, |el| el.bg(theme.primary))
                            .text_size(px(TEXT_TINY + 0.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if on {
                                theme.on_primary
                            } else {
                                theme.text_muted
                            })
                            .child(t.label())
                            .on_click(cx.listener(move |this, _, _, cx| this.set_fun_tab(t, cx)))
                    }),
            );
        let close = div()
            .id("fun-close")
            .size(px(24.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .map(|el| highlight::button(el, &theme))
            .child(sf_symbols::glyph("xmark", 11.0, theme.text_muted))
            .on_click(cx.listener(|this, _, _, cx| this.close_fun(cx)));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .pt(px(10.0))
            .px(px(PAD))
            .child(tabs)
            .child(close);
        let body = match tab {
            FunTab::Emoji => self.render_emoji_tab(&theme, cx),
            FunTab::Stickers => self.render_stickers_tab(&theme, cx),
            FunTab::Gifs => self.render_gifs_tab(&theme, cx),
        };
        let card = div()
            .id("fun-picker")
            .debug_selector(|| "fun-picker".into())
            .flex()
            .flex_col()
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .rounded(px(PANEL_RADIUS))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            .overflow_hidden()
            // The card covers transcript rows; without its own occluding hitbox a tap on a
            // tab also reached the picture under it and opened the media viewer (rig
            // shots, 2026-10-29). Same for the wheel: scrolling the grid must not move
            // the transcript.
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body);
        card.into_any_element()
    }

    fn search_field(&self, theme: &Theme) -> AnyElement {
        div()
            .id("fun-search")
            .h(px(SEARCH_H))
            .mx(px(PAD))
            .mt(px(10.0))
            .px(px(10.0))
            .rounded_full()
            .bg(theme.field)
            .border_1()
            .border_color(theme.field_border)
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_text()
            .child(sf_symbols::glyph("magnifyingglass", 12.0, theme.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(TEXT_SMALL))
                    .child(self.fun.search.clone()),
            )
            .into_any_element()
    }

    fn footer(theme: &Theme, left: AnyElement, right: AnyElement) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(32.0))
            .px(px(PAD))
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(TEXT_TINY - 1.0))
            .text_color(theme.text_muted)
            .child(left)
            .child(right)
            .into_any_element()
    }

    // ----- Emoji tab ------------------------------------------------------------

    fn render_emoji_tab(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let query = self.fun.search.read(cx).text().trim().to_lowercase();
        let tone = self.fun.tone;
        let cell = |id: ElementId, glyph: String, theme: &Theme| {
            div()
                .id(id)
                .size(px(EMOJI_CELL))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .map(|el| highlight::button(el, theme))
                .child(div().text_size(px(EMOJI_GLYPH)).child(glyph))
        };
        let section = |label: &str, theme: &Theme| {
            div()
                .text_size(px(TEXT_TINY - 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_faint)
                .mt(px(8.0))
                .mb(px(2.0))
                .child(label.to_uppercase())
        };
        let mut grid = glass::contained_y_scroll("fun-emoji-scroll")
            .flex_1()
            .min_h_0()
            .px(px(PAD));
        let mut rows: Vec<(String, String)> = Vec::new(); // (id suffix, glyph)
        if !query.is_empty() {
            let hits = search_emoji(&query, 64);
            grid = grid.child(section(
                if hits.is_empty() {
                    "No emoji match"
                } else {
                    "Results"
                },
                theme,
            ));
            for (i, e) in hits.iter().enumerate() {
                rows.push((format!("hit-{i}"), with_tone(e, tone)));
            }
        } else {
            if !self.fun.recents.is_empty() {
                grid = grid.child(section("Recents", theme));
                let recents: Vec<String> = self.fun.recents.clone();
                grid = grid.child(div().flex().flex_wrap().children(
                    recents.into_iter().enumerate().map(|(i, g)| {
                        let picked = g.clone();
                        cell(
                            ElementId::NamedInteger("fun-recent".into(), i as u64),
                            g,
                            theme,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.pick_emoji(picked.clone(), cx)),
                        )
                    }),
                ));
            }
            let (cat_id, cat_label, _) = CATEGORIES[self.fun.category.min(CATEGORIES.len() - 1)];
            grid = grid.child(section(cat_label, theme));
            for (i, e) in emoji_list().iter().filter(|e| e.c == cat_id).enumerate() {
                rows.push((format!("cat-{i}"), with_tone(e, tone)));
            }
        }
        grid = grid.child(
            div()
                .flex()
                .flex_wrap()
                .pb(px(8.0))
                .children(rows.into_iter().map(|(suffix, g)| {
                    let picked = g.clone();
                    cell(
                        ElementId::Name(format!("fun-emoji-{suffix}").into()),
                        g,
                        theme,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.pick_emoji(picked.clone(), cx)),
                    )
                })),
        );
        let strip =
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .children(CATEGORIES.iter().enumerate().map(|(i, (_, label, glyph))| {
                    let on = i == self.fun.category && query.is_empty();
                    div()
                        .id(ElementId::NamedInteger("fun-cat".into(), i as u64))
                        .w(px(28.0))
                        .h(px(26.0))
                        .rounded(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(on, |el| el.bg(theme.surface_2))
                        .map(|el| highlight::button(el, theme))
                        .tooltip({
                            let label = *label;
                            move |_, cx| cx.new(|_| super::TextTip(label.into())).into()
                        })
                        .child(
                            div()
                                .text_size(px(14.0))
                                .opacity(if on { 1.0 } else { 0.55 })
                                .child(*glyph),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.fun.category = i;
                            this.fun.search.update(cx, |i, cx| i.clear(cx));
                            cx.notify();
                        }))
                }));
        let tone_swatch = div()
            .id("fun-tone")
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .tooltip(|_, cx| cx.new(|_| super::TextTip("Skin tone".into())).into())
            .child(
                div()
                    .text_size(px(TEXT_TINY - 1.0))
                    .text_color(theme.text_faint)
                    .child("Tone"),
            )
            .child(
                div()
                    .size(px(12.0))
                    .rounded_full()
                    .bg(gpui::rgb(TONE_SWATCH[tone.min(TONE_SWATCH.len() - 1)]))
                    .border_1()
                    .border_color(theme.border_strong),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.fun.tone = (this.fun.tone + 1) % (TONES.len() + 1);
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.search_field(theme))
            .child(grid)
            .child(Self::footer(
                theme,
                strip.into_any_element(),
                tone_swatch.into_any_element(),
            ))
            .into_any_element()
    }

    // ----- Stickers tab ---------------------------------------------------------

    fn render_stickers_tab(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let relay_packs: Vec<StickerPack> = self
            .state
            .read(cx)
            .sticker_packs
            .clone()
            .flatten()
            .unwrap_or_default();
        let query = self.fun.search.read(cx).text().trim().to_lowercase();
        // Pack switcher: starter · each relay pack (the relay's copy of the starter's pack is
        // the full set, so it replaces the starter tile when present).
        let mut switcher = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(PAD - 4.0))
            .pt(px(8.0));
        let starter_in_relay = relay_packs
            .iter()
            .position(|p| p.id == fun_starter::PACK_ID);
        let mut options: Vec<(PackSel, String, Option<Arc<RenderImage>>)> = Vec::new();
        if starter_in_relay.is_none() {
            let img = fun_starter::STARTER
                .first()
                .and_then(|s| self.fun.starter_image(s));
            options.push((PackSel::Starter, "Fluent 3D".into(), img));
        }
        for (i, p) in relay_packs.iter().enumerate() {
            let icon = p
                .items
                .first()
                .and_then(|it| {
                    self.state
                        .read(cx)
                        .card_thumbnail(&sticker_key(&p.id, &it.id))
                })
                .or_else(|| {
                    (p.id == fun_starter::PACK_ID)
                        .then(|| {
                            fun_starter::STARTER
                                .first()
                                .and_then(|s| self.fun.starter_image(s))
                        })
                        .flatten()
                });
            options.push((PackSel::Relay(i), p.name.clone(), icon));
        }
        // A selection that no longer exists (packs arrived) falls back to the first.
        let selected = match self.fun.pack {
            PackSel::Starter => match starter_in_relay {
                Some(i) => PackSel::Relay(i),
                None => PackSel::Starter,
            },
            PackSel::Relay(i) if i >= relay_packs.len() => {
                options.first().map(|o| o.0).unwrap_or(PackSel::Starter)
            }
            other => other,
        };
        for (i, (sel, name, icon)) in options.iter().enumerate() {
            let on = *sel == selected;
            let sel = *sel;
            let name = name.clone();
            let mut tile = div()
                .id(ElementId::NamedInteger("fun-pack".into(), i as u64))
                .size(px(40.0))
                .rounded(px(10.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.surface_2))
                .map(|el| highlight::button(el, theme))
                .tooltip(move |_, cx| cx.new(|_| super::TextTip(name.clone().into())).into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.fun.pack = sel;
                    this.fun.sticker_pages = 1;
                    cx.notify();
                }));
            tile = match icon {
                Some(image) => tile.child(
                    img(image.clone())
                        .size(px(28.0))
                        .object_fit(ObjectFit::Contain),
                ),
                None => tile.child(sf_symbols::glyph("face.smiling", 16.0, theme.text_muted)),
            };
            switcher = switcher.child(tile);
        }

        // The grid for the selected pack.
        let (_pack, attribution, tiles): (StickerPack, String, Vec<AnyElement>) = match selected {
            PackSel::Starter => {
                let pack = starter_pack();
                let mut tiles = Vec::new();
                for (i, s) in fun_starter::STARTER
                    .iter()
                    .filter(|s| query.is_empty() || s.name.contains(&query))
                    .enumerate()
                {
                    let image = self.fun.starter_image(s);
                    let item = StickerItem {
                        id: s.id.into(),
                        name: s.name.into(),
                        emoji: s.emoji.into(),
                        mime: "image/png".into(),
                        path: format!("/stickers/{}/{}", fun_starter::PACK_ID, s.id),
                    };
                    let bytes = Arc::new(s.png.to_vec());
                    let pack_for = pack.clone();
                    let name: String = s.name.into();
                    tiles.push(
                        sticker_cell(
                            ElementId::NamedInteger("fun-sticker".into(), i as u64),
                            image,
                            name,
                            theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_sticker(pack_for.clone(), item.clone(), bytes.clone(), cx)
                        }))
                        .into_any_element(),
                    );
                }
                (pack, "Fluent Emoji 3D · Microsoft · MIT".into(), tiles)
            }
            PackSel::Relay(i) => {
                let pack = relay_packs[i].clone();
                let shown: Vec<StickerItem> = pack
                    .items
                    .iter()
                    .filter(|it| query.is_empty() || it.name.contains(&query))
                    .take(STICKER_PAGE * self.fun.sticker_pages)
                    .cloned()
                    .collect();
                // Ask the relay for what the grid shows (once per key per session).
                let keys: Vec<String> = shown
                    .iter()
                    .map(|it| sticker_key(&pack.id, &it.id))
                    .collect();
                self.state.update(cx, |s, _| {
                    for k in keys {
                        s.request_fun_thumbnail(k);
                    }
                });
                let mut tiles = Vec::new();
                for (j, item) in shown.into_iter().enumerate() {
                    let key = sticker_key(&pack.id, &item.id);
                    let (image, bytes) = {
                        let s = self.state.read(cx);
                        (s.card_thumbnail(&key), s.sticker_bytes(&key))
                    };
                    let image = image.or_else(|| {
                        // The starter has this one bundled: show it while the relay copy loads.
                        fun_starter::STARTER
                            .iter()
                            .find(|st| pack.id == fun_starter::PACK_ID && st.id == item.id)
                            .and_then(|st| self.fun.starter_image(st))
                    });
                    let bytes = bytes.or_else(|| {
                        fun_starter::STARTER
                            .iter()
                            .find(|st| pack.id == fun_starter::PACK_ID && st.id == item.id)
                            .map(|st| Arc::new(st.png.to_vec()))
                    });
                    let pack_for = pack.clone();
                    let name = item.name.clone();
                    let mut cell = sticker_cell(
                        ElementId::NamedInteger("fun-sticker".into(), j as u64),
                        image,
                        name,
                        theme,
                    );
                    if let Some(bytes) = bytes {
                        cell = cell.on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_sticker(pack_for.clone(), item.clone(), bytes.clone(), cx)
                        }));
                    }
                    tiles.push(cell.into_any_element());
                }
                let more = pack
                    .items
                    .iter()
                    .filter(|it| query.is_empty() || it.name.contains(&query))
                    .count()
                    > STICKER_PAGE * self.fun.sticker_pages;
                if more {
                    tiles.push(
                        div()
                            .id("fun-sticker-more")
                            .w_full()
                            .h(px(32.0))
                            .mt(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .map(|el| highlight::button(el, theme))
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_muted)
                            .child("More")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.fun.sticker_pages += 1;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                let attribution = pack.attribution.clone();
                (pack, attribution, tiles)
            }
        };
        let grid = glass::contained_y_scroll("fun-sticker-scroll")
            .flex_1()
            .min_h_0()
            .px(px(PAD))
            .pt(px(6.0))
            .child(div().flex().flex_wrap().children(tiles));
        let hint = if self.state.read(cx).sticker_packs.is_none() && relay_packs.is_empty() {
            "Loading packs…"
        } else {
            "Tap to send"
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(switcher)
            .child(self.search_field(theme))
            .child(grid)
            .child(Self::footer(
                theme,
                div().child(attribution).into_any_element(),
                div().child(hint).into_any_element(),
            ))
            .into_any_element()
    }

    // ----- GIFs tab -----------------------------------------------------------------

    fn render_gifs_tab(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = self.fun.gif_query.clone();
        let chips = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(PAD))
            .pt(px(8.0))
            .overflow_hidden()
            .children(GIF_CHIPS.iter().enumerate().map(|(i, term)| {
                let is_trending = i == 0;
                let on = if is_trending {
                    current.is_empty()
                } else {
                    current.eq_ignore_ascii_case(term)
                };
                let term_s: String = if is_trending {
                    String::new()
                } else {
                    (*term).into()
                };
                div()
                    .id(ElementId::NamedInteger("fun-gif-chip".into(), i as u64))
                    .h(px(24.0))
                    .px(px(11.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .bg(if on { theme.primary } else { theme.surface_2 })
                    .text_size(px(TEXT_TINY))
                    .text_color(if on { theme.on_primary } else { theme.text })
                    .child(*term)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let t = term_s.clone();
                        this.fun
                            .search
                            .update(cx, |i, cx| i.set_text(t.clone(), cx));
                        this.fun.gif_query = t.clone();
                        this.state.update(cx, |s, cx| s.gif_search(t, cx));
                        cx.notify();
                    }))
            }));
        let results = self.state.read(cx).gif_results.clone();
        let body: AnyElement = match results {
            None => centred_note(theme, "Loading…", None),
            Some((_, GifPage::Unavailable)) => centred_note(
                theme,
                "GIFs are not set up yet.",
                Some("The server has no GIF provider key."),
            ),
            Some((_, GifPage::Busy)) => {
                centred_note(theme, "GIFs are busy, try again shortly.", None)
            }
            Some((_, GifPage::Failed)) => centred_note(
                theme,
                "GIFs could not be loaded.",
                Some("Check the connection and try again."),
            ),
            Some((_, GifPage::Items { items, next })) if items.is_empty() => {
                let _ = next;
                centred_note(theme, "No GIFs for that.", None)
            }
            Some((_, GifPage::Items { items, next })) => {
                let mut cols: Vec<Vec<AnyElement>> = (0..GIF_COLS).map(|_| Vec::new()).collect();
                let mut heights = [0.0f32; GIF_COLS];
                for (i, gif) in items.into_iter().enumerate() {
                    let ratio = if gif.preview.width > 0 && gif.preview.height > 0 {
                        gif.preview.height as f32 / gif.preview.width as f32
                    } else {
                        1.0
                    };
                    let h = (GIF_COL_W * ratio).clamp(60.0, 220.0);
                    let col = heights
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(c, _)| c)
                        .unwrap_or(0);
                    heights[col] += h + 6.0;
                    let key = format!("{URL_KEY_PREFIX}{}", gif.preview.url);
                    let image = self.state.read(cx).card_thumbnail(&key);
                    let title = gif.title.clone();
                    let mut tile = div()
                        .id(ElementId::NamedInteger("fun-gif".into(), i as u64))
                        .w(px(GIF_COL_W))
                        .h(px(h))
                        .rounded(px(10.0))
                        .overflow_hidden()
                        .bg(theme.surface_2)
                        .cursor_pointer()
                        .tooltip(move |_, cx| {
                            cx.new(|_| super::TextTip(title.clone().into())).into()
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.pick_gif(gif.clone(), cx)),
                        );
                    if let Some(image) = image {
                        tile = tile.child(
                            img(image)
                                .w(px(GIF_COL_W))
                                .h(px(h))
                                .object_fit(ObjectFit::Cover),
                        );
                    }
                    cols[col].push(tile.into_any_element());
                }
                let mut grid = glass::contained_y_scroll("fun-gif-scroll")
                    .flex_1()
                    .min_h_0()
                    .px(px(PAD))
                    .pt(px(8.0));
                grid = grid.child(
                    div().flex().gap(px(6.0)).items_start().children(
                        cols.into_iter()
                            .map(|c| div().flex().flex_col().gap(px(6.0)).children(c)),
                    ),
                );
                if next.is_some() {
                    grid = grid.child(
                        div()
                            .id("fun-gif-more")
                            .w_full()
                            .h(px(32.0))
                            .my(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .map(|el| highlight::button(el, theme))
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_muted)
                            .child("More")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.gif_more(cx));
                            })),
                    );
                }
                grid.into_any_element()
            }
        };
        let powered = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .child(
                div()
                    .w(px(10.0))
                    .h(px(14.0))
                    .rounded(px(2.0))
                    .bg(gpui::rgb(0x6C5CE7)),
            )
            .child("Powered by KLIPY");
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.search_field(theme))
            .child(chips)
            .child(body)
            .child(Self::footer(
                theme,
                powered.into_any_element(),
                div()
                    .child("Searches are sent to KLIPY.")
                    .into_any_element(),
            ))
            .into_any_element()
    }
}

/// One sticker tile: the picture (or a blank while it loads) with the name as a tooltip.
fn sticker_cell(
    id: ElementId,
    image: Option<Arc<RenderImage>>,
    name: String,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let mut cell = div()
        .id(id)
        // A hair under a quarter of the row so four cells wrap per line (flex_wrap rounds).
        .w(px((WIDTH - 2.0 * PAD - 8.0) / STICKER_COLS as f32))
        .h(px(STICKER_CELL))
        .rounded(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .map(|el| highlight::button(el, theme))
        .tooltip(move |_, cx| cx.new(|_| super::TextTip(name.clone().into())).into());
    cell = match image {
        Some(image) => cell.child(
            img(image)
                .size(px(STICKER_IMG))
                .object_fit(ObjectFit::Contain),
        ),
        None => cell.child(
            div()
                .size(px(STICKER_IMG * 0.6))
                .rounded(px(10.0))
                .bg(theme.surface_2),
        ),
    };
    cell
}

/// The GIF tab's one-line state (unavailable / busy / failed / empty).
fn centred_note(theme: &Theme, line: &str, detail: Option<&str>) -> AnyElement {
    let mut col = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .px(px(PAD))
        .child(
            div()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(line.to_owned()),
        );
    if let Some(d) = detail {
        col = col.child(
            div()
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_faint)
                .child(d.to_owned()),
        );
    }
    col.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_list_loads_with_categories_and_tones() {
        let list = emoji_list();
        assert!(list.len() > 1800, "{}", list.len());
        let grin = list
            .iter()
            .find(|e| e.n == "grinning face")
            .expect("grinning face");
        assert_eq!(grin.e, "😀");
        assert_eq!(grin.c, "smileys");
        assert_eq!(grin.t, 0);
        let thumbs = list.iter().find(|e| e.n == "thumbs up").expect("thumbs up");
        assert_eq!(thumbs.t, 1);
        for (id, _, _) in CATEGORIES {
            assert!(list.iter().any(|e| e.c == *id), "category {id} is empty");
        }
    }

    #[test]
    fn skin_tone_goes_after_the_first_scalar() {
        let thumbs = Emoji {
            e: "👍".into(),
            n: "thumbs up".into(),
            c: "people".into(),
            t: 1,
        };
        assert_eq!(with_tone(&thumbs, 3), "👍🏽");
        assert_eq!(with_tone(&thumbs, 0), "👍");
        let wave_zwj = Emoji {
            e: "🧑\u{200D}🚀".into(),
            n: "astronaut".into(),
            c: "people".into(),
            t: 1,
        };
        assert_eq!(with_tone(&wave_zwj, 1), "🧑🏻\u{200D}🚀");
        let smile = Emoji {
            e: "😀".into(),
            n: "grinning face".into(),
            c: "smileys".into(),
            t: 0,
        };
        assert_eq!(with_tone(&smile, 5), "😀");
    }

    #[test]
    fn search_prefers_name_starts_then_words() {
        let hits = search_emoji("thumb", 10);
        assert!(hits.iter().any(|e| e.n == "thumbs up"));
        let hits = search_emoji("heart", 64);
        assert!(hits.first().unwrap().n.starts_with("heart"));
        assert!(hits.iter().any(|e| e.n == "red heart"));
        assert!(search_emoji("", 10).is_empty());
    }

    #[test]
    fn starter_pack_is_bundled_and_decodes() {
        assert_eq!(fun_starter::STARTER.len(), 48);
        let first = &fun_starter::STARTER[0];
        assert_eq!(first.id, "grinning-face");
        assert!(crate::app::decode_image(first.png).is_some());
    }
}
