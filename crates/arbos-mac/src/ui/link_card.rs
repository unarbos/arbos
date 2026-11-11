//! Link previews ([feedback link-embeds-mac-2026-10-27], #2065): the card a `link_preview`
//! part draws inside its bubble — the code / chart card's frame (`code_bg`, hairline, radius
//! 10, 10 pt padding, at most [`CARD_MAX_WIDTH`] wide). Two densities, decided by the relay:
//! **compact** = 56 pt art on the left, title · subtitle · footer on the right; **expanded** =
//! the picture on top at 16:9 (▶ and the duration on a video), the text under it. Every card
//! opens out: a click is the link's click (`LinkUi::on_open` → the app's link opener, where
//! #1522's confirm slots in). On an own message the hover shows an × top-right (Remove preview,
//! for everyone). The footer's small square is the shared link-icon tile (`link_tile`): the
//! host's bundled brand mark, else the relay's `siteIcon` picture, else the relay's host icon,
//! else the host's initial — the card itself fetches nothing from the site.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, AnyElement, App, Div, FontWeight, Hsla, ObjectFit, RenderImage,
    SharedString, Window,
};

use crate::model::LinkPreview;

use super::link_tile::{self, Brand, Icon, Tone};
use super::sf_symbols;
use super::theme::Theme;

pub const CARD_RADIUS: f32 = 10.0;
pub const CARD_PAD: f32 = 10.0;
pub const CARD_MAX_WIDTH: f32 = 400.0;
pub const ART: f32 = 56.0;
pub const ART_RADIUS: f32 = 6.0;
/// The expanded picture's height at the card's full width (16:9 of 400 → 225; a narrower
/// bubble keeps the height, the picture is cropped — never stretched).
pub const MEDIA_HEIGHT: f32 = 225.0;
pub const PLAY_DISC: f32 = 44.0;
pub const TILE: f32 = 13.0;
pub const REMOVE_DISC: f32 = 20.0;
const TITLE: f32 = 13.0;
const SUB: f32 = 12.0;
const FOOT: f32 = 11.0;

/// The transcript's wiring for the cards: the picture for a blob key (`None` while loading),
/// the link opener and, on messages the user may edit, Remove preview.
#[derive(Clone)]
pub struct LinkUi {
    #[allow(clippy::type_complexity)]
    pub thumbnail: Rc<dyn Fn(&str) -> Option<Arc<RenderImage>>>,
    /// The relay's icon for a host (`AppState::link_icon`), for hosts with no bundled mark.
    #[allow(clippy::type_complexity)]
    pub host_icon: Rc<dyn Fn(&str) -> Option<(Arc<RenderImage>, Tone)>>,
    #[allow(clippy::type_complexity)]
    pub on_open: Rc<dyn Fn(String, &mut Window, &mut App)>,
    #[allow(clippy::type_complexity)]
    pub on_remove: Rc<dyn Fn(String, &mut Window, &mut App)>,
}

/// Where the card sits, for its colours and controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// My own grey bubble (white text) rather than an agent's / another person's.
    pub own: bool,
    /// The × (Remove preview) may show: my message, or one I moderate.
    pub removable: bool,
}

fn muted(theme: &Theme, own: bool) -> Hsla {
    if own {
        Theme::with_alpha(theme.bubble_text(), 0.66)
    } else {
        theme.text_muted
    }
}

fn faint(theme: &Theme, own: bool) -> Hsla {
    if own {
        Theme::with_alpha(theme.bubble_text(), 0.5)
    } else {
        theme.text_faint
    }
}

fn ink(theme: &Theme, own: bool) -> Hsla {
    if own {
        theme.bubble_text()
    } else {
        theme.text
    }
}

/// The host a card stands for: `siteHost` from the relay, else the canonical URL's host.
fn host_of(preview: &LinkPreview) -> String {
    preview
        .site_host
        .clone()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            let url = if preview.canonical_url.is_empty() {
                &preview.url
            } else {
                &preview.canonical_url
            };
            url.split("://")
                .nth(1)
                .and_then(|r| r.split(['/', '?', '#']).next())
                .map(|h| h.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_default()
}

/// The footer's tile: the shared link-icon tile at [`TILE`] pt — a bundled brand mark for a
/// known host, else the relay's `siteIcon` picture on the part, else the relay's host icon,
/// else the host's initial.
pub fn tile_slot(preview: &LinkPreview, ui: Option<&Rc<LinkUi>>, theme: &Theme) -> Div {
    let host = host_of(preview);
    let site_icon = preview
        .site_icon
        .as_ref()
        .filter(|i| !i.blob_ref.is_empty())
        .and_then(|i| ui.and_then(|ui| (ui.thumbnail)(&i.blob_ref)));
    let host_icon = ui.and_then(|ui| (ui.host_icon)(&host));
    let tile = if let Some(brand) = Brand::for_host(&host) {
        link_tile::tile(Icon::Brand(brand), TILE, theme)
    } else if let Some(image) = site_icon.as_ref() {
        link_tile::tile(Icon::Picture(image, Tone::Neutral), TILE, theme)
    } else if let Some((image, tone)) = host_icon.as_ref() {
        link_tile::tile(Icon::Picture(image, *tone), TILE, theme)
    } else {
        link_tile::tile(Icon::Initial(host.clone()), TILE, theme)
    };
    tile.debug_selector(|| "link-tile".into())
}

fn one_line(text: &str, size: f32, weight: FontWeight, color: Hsla) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(size + 4.0))
        .font_weight(weight)
        .text_color(color)
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(text.to_owned())
}

fn clamped(text: &str, size: f32, weight: FontWeight, color: Hsla, lines: usize) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(size + 5.0))
        .font_weight(weight)
        .text_color(color)
        .overflow_hidden()
        .line_clamp(lines)
        .child(text.to_owned())
}

/// "Spotify · Song · 4:03" after the tile slot.
fn footer(preview: &LinkPreview, theme: &Theme, own: bool, ui: Option<&Rc<LinkUi>>) -> Div {
    let mut parts: Vec<String> = Vec::new();
    if !preview.provider_name.trim().is_empty() {
        parts.push(preview.provider_name.trim().to_owned());
    }
    parts.extend(preview.facts.iter().map(|f| f.trim().to_owned()));
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .pt(px(4.0))
        .text_size(px(FOOT))
        .line_height(px(FOOT + 3.0))
        .text_color(faint(theme, own))
        .overflow_hidden()
        .whitespace_nowrap()
        .debug_selector(|| "link-footer".into())
        .child(tile_slot(preview, ui, theme));
    if let Some(diff) = &preview.diff {
        row = row
            .child(
                div()
                    .text_color(theme.online)
                    .child(format!("+{}", diff.additions)),
            )
            .child(
                div()
                    .text_color(theme.danger) // error-ui: exception diff-deletions-count
                    .child(format!("−{}", diff.deletions)),
            );
        if !parts.is_empty() {
            row = row.child(div().opacity(0.7).child("·"));
        }
    }
    row.child(
        div()
            .overflow_hidden()
            .text_ellipsis()
            .child(parts.join(" · ")),
    )
}

fn state_pill(state: &str, theme: &Theme) -> Div {
    let (label, color) = match state {
        "open" => ("Open", theme.online),
        "merged" => ("Merged", gpui::rgb(0x8957e5).into()),
        "draft" => ("Draft", theme.text_muted),
        _ => ("Closed", theme.danger),
    };
    div()
        .flex_shrink_0()
        .px(px(7.0))
        .rounded(px(999.0))
        .bg(Theme::with_alpha(color, 0.18))
        .text_size(px(FOOT))
        .line_height(px(FOOT + 7.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .debug_selector(|| "link-state".into())
        .child(label)
}

/// The picture area: the thumbnail when loaded, a quiet fill while it loads.
fn picture(
    key: &str,
    ui: Option<&Rc<LinkUi>>,
    theme: &Theme,
    size: impl FnOnce(Div) -> Div,
    fit: ObjectFit,
) -> Div {
    let image = ui.and_then(|ui| (ui.thumbnail)(key));
    let mut el = size(
        div()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(Theme::with_alpha(theme.text_muted, 0.14)),
    );
    if let Some(image) = image {
        el = el.child(img(image).size_full().object_fit(fit));
    }
    el
}

/// The card.
#[allow(clippy::too_many_arguments)]
pub fn card(
    row_id: &str,
    message_id: &str,
    preview: &LinkPreview,
    theme: &Theme,
    ui: Option<&Rc<LinkUi>>,
    place: Place,
) -> AnyElement {
    let own = place.own;
    let group = SharedString::from(format!("{row_id}#link-group"));
    let url = if preview.canonical_url.is_empty() {
        preview.url.clone()
    } else {
        preview.canonical_url.clone()
    };
    let open_ui = ui.cloned();
    let open_url = url.clone();
    // The card is a filled control: the one hover / pressed wash composited over its
    // `code_bg` fill (`ui::highlight::washed`), like the pills on `surface_2`.
    let card = div()
        .id(SharedString::from(format!("{row_id}#link")))
        .relative()
        .flex()
        .flex_col()
        // A definite width, not `w_full` capped at 400: the bubble shrink-wraps its content, and
        // Taffy sizes a percentage-wide child at its unwrapped text width before the cap
        // applies — a title that wraps at 400 pt came out a line short and the footer was
        // clipped (v0.6.310 proof shot, the YouTube row). `max_w_full` keeps a narrower bubble
        // (window at its 900 pt minimum) from overflowing.
        .w(px(CARD_MAX_WIDTH))
        .max_w_full()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .cursor_pointer()
        .debug_selector(|| "link-card".into())
        .group(group.clone());
    let mut card =
        super::highlight::washed(card, theme.code_bg, theme).on_click(move |_, window, cx| {
            if let Some(ui) = &open_ui {
                (ui.on_open)(open_url.clone(), window, cx);
            }
        });

    let is_post = preview.kind == "post";
    let expanded = preview.expanded();

    if expanded {
        if let Some(image) = &preview.image {
            let mut media = picture(
                &image.blob_ref,
                ui,
                theme,
                |d| d.w_full().h(px(MEDIA_HEIGHT)).relative(),
                ObjectFit::Cover,
            )
            .debug_selector(|| "link-media".into());
            if preview.kind == "video" {
                media = media.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(PLAY_DISC))
                                .rounded(px(PLAY_DISC / 2.0))
                                .bg(Theme::with_alpha(gpui::black(), 0.62))
                                .border_1()
                                .border_color(Theme::with_alpha(gpui::white(), 0.18))
                                .flex()
                                .items_center()
                                .justify_center()
                                .pl(px(3.0))
                                .debug_selector(|| "link-play".into())
                                .child(sf_symbols::glyph("play.fill", 22.0, gpui::white())),
                        ),
                );
            }
            if let Some(d) = &preview.duration {
                media = media.child(
                    div()
                        .absolute()
                        .right(px(8.0))
                        .bottom(px(8.0))
                        .px(px(5.0))
                        .rounded(px(4.0))
                        .bg(Theme::with_alpha(gpui::black(), 0.72))
                        .text_size(px(FOOT))
                        .line_height(px(FOOT + 6.0))
                        .text_color(gpui::white())
                        .debug_selector(|| "link-duration".into())
                        .child(d.clone()),
                );
            }
            card = card.child(media);
        }
    }

    // The text column: compact puts the art beside it; expanded puts it under the picture.
    let mut text = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .debug_selector(|| "link-text".into());
    if is_post {
        // X / Bluesky: name · @handle, the post text, the date.
        let initials: String = preview
            .title
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        text = text.child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(32.0))
                        .rounded(px(16.0))
                        .bg(Theme::with_alpha(theme.accent, 0.35))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(gpui::white())
                        .child(initials),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(one_line(
                            &preview.title,
                            TITLE,
                            FontWeight::SEMIBOLD,
                            ink(theme, own),
                        ))
                        .children(
                            preview
                                .subtitle
                                .as_deref()
                                .map(|s| one_line(s, SUB, FontWeight::NORMAL, muted(theme, own))),
                        ),
                ),
        );
        if let Some(d) = &preview.description {
            text =
                text.child(clamped(d, TITLE, FontWeight::NORMAL, ink(theme, own), 8).mt(px(6.0)));
        }
    } else {
        if preview.kind == "pull_request" || preview.kind == "issue" || preview.kind == "gist" {
            if let Some(s) = &preview.subtitle {
                text = text.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .mb(px(2.0))
                        .text_size(px(SUB))
                        .line_height(px(SUB + 4.0))
                        .text_color(muted(theme, own))
                        .child(tile_slot(preview, ui, theme))
                        .child(div().overflow_hidden().text_ellipsis().child(s.clone())),
                );
            }
            text = text.child(clamped(
                &preview.title,
                TITLE,
                FontWeight::SEMIBOLD,
                ink(theme, own),
                2,
            ));
            let mut meta = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .mt(px(4.0))
                .text_size(px(SUB))
                .line_height(px(SUB + 4.0))
                .text_color(muted(theme, own))
                .overflow_hidden();
            if let Some(s) = &preview.state {
                meta = meta.child(state_pill(s, theme));
            }
            if let Some(d) = &preview.description {
                meta = meta.child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(d.clone()),
                );
            }
            text = text.child(meta);
        } else {
            text = text.child(clamped(
                &preview.title,
                TITLE,
                FontWeight::SEMIBOLD,
                ink(theme, own),
                if expanded { 2 } else { 1 },
            ));
            if let Some(s) = &preview.subtitle {
                text = text.child(one_line(s, SUB, FontWeight::NORMAL, muted(theme, own)));
            }
            if expanded || preview.subtitle.is_none() {
                if let Some(d) = &preview.description {
                    text = text.child(
                        clamped(d, SUB, FontWeight::NORMAL, muted(theme, own), 2).mt(px(2.0)),
                    );
                }
            }
        }
    }
    text = text.child(footer(preview, theme, own, ui));

    let mut body = div()
        .flex()
        .gap(px(10.0))
        .p(px(CARD_PAD))
        .items_stretch()
        .debug_selector(|| "link-body".into());
    if !expanded && !is_post {
        if let Some(image) = &preview.image {
            body = body.child(
                picture(
                    &image.blob_ref,
                    ui,
                    theme,
                    |d| d.size(px(ART)).rounded(px(ART_RADIUS)),
                    ObjectFit::Cover,
                )
                .debug_selector(|| "link-art".into()),
            );
        }
    }
    card = card.child(body.child(text));

    if place.removable {
        let mid = message_id.to_owned();
        let remove_ui = ui.cloned();
        card = card.child(
            div()
                .id(SharedString::from(format!("{row_id}#link-remove")))
                .absolute()
                .top(px(6.0))
                .right(px(6.0))
                .size(px(REMOVE_DISC))
                .rounded(px(REMOVE_DISC / 2.0))
                .opacity(0.0)
                .group_hover(group.clone(), |s| s.opacity(1.0))
                .bg(Theme::with_alpha(gpui::black(), 0.55))
                .border_1()
                .border_color(Theme::with_alpha(gpui::white(), 0.14))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .debug_selector(|| "link-remove".into())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    if let Some(ui) = &remove_ui {
                        (ui.on_remove)(mid.clone(), window, cx);
                    }
                })
                .child(sf_symbols::glyph(
                    "xmark",
                    10.0,
                    Theme::with_alpha(gpui::white(), 0.85),
                )),
        );
    }
    card.into_any_element()
}
