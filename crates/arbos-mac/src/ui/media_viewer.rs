//! The media viewer (feedback media-viewer-2026-10-23, #1381): one lightbox
//! for every chat kind, opened from an attachment card or a Library tile.
//! Black backdrop over the window; the album (the chat's pictures and videos
//! in order) pages with ← → or the chevrons; × / Close / Esc / backdrop /
//! swipe-down (scroll) close it and return to the chat
//! ([feedback app-01M5N9BJNZX8T1EQCX3RAB7GXD]). Caption top-left (who, when,
//! file), counter top-centre, Close · Download · Forward pills at the bottom.
//! Video shows its poster with Play (QuickTime Player); a file shows its tile
//! with Quick Look. Forward opens a picker of DMs, agents and groups; one
//! click sends the item there as a new message. The same picker serves the
//! bubble menu's Forward (feedback app-01M5MXRZ7609S70P9S9P2RZDDF), over the
//! chat instead of the lightbox.

use gpui::{
    actions, div, img, prelude::*, px, AnyElement, Context, Div, ElementId, FocusHandle,
    FontWeight, Hsla, MouseButton, ObjectFit, Stateful,
};

use crate::app::{AppState, ForwardTarget, ViewerItem};
use crate::model::AttachmentKind;
use crate::ui::avatars;
use crate::ui::icons::icon;
use crate::ui::modal::modal_overlay;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL, TEXT_TINY};

actions!(media_viewer, [Prev, Next]);

/// Backdrop over the window.
const BACKDROP: f32 = 0.88;
/// The round controls (×, chevrons): mock-mac 36 pt.
pub const CONTROL: f32 = 36.0;
/// Download / Forward pills: 34 pt tall, 13 pt medium text.
pub const PILL_HEIGHT: f32 = 34.0;
/// Inset from the window edge to the controls.
const EDGE: f32 = 28.0;
/// The caption starts clear of the window's traffic lights (which macOS draws
/// over the overlay): the header's `TRAFFIC_LIGHTS_INSET` plus the edge gap.
const CAPTION_LEFT: f32 = crate::ui::shell::TRAFFIC_LIGHTS_INSET + 12.0;
/// The picture's room: top bar and bottom pills stay clear of it.
const PICTURE_TOP: f32 = 70.0;
const PICTURE_BOTTOM: f32 = 90.0;
/// Trackpad / wheel swipe-down past this many pixels closes the viewer
/// (same idea as the iPhone's swipe-down dismiss).
const DISMISS_SCROLL_PX: f32 = 48.0;
/// Forward picker card.
const PICKER_WIDTH: f32 = 360.0;
const PICKER_HEIGHT: f32 = 420.0;
const PICKER_ROW: f32 = 52.0;
/// The picker's search field (feedback forward-sheet-focus-2026-10-24): 28 pt
/// tall, 13 pt text — the row-name size, like Grok's palette field.
pub const PICKER_FIELD_HEIGHT: f32 = 28.0;
pub const PICKER_FIELD_FONT: f32 = 13.0;

fn white(alpha: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: alpha,
    }
}

/// "Today at 2:14 PM", "Yesterday at 9:03 AM", "Oct 12 at 4:40 PM".
pub fn when_label(ms: u64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(at) = Local.timestamp_millis_opt(ms as i64).single() else {
        return String::new();
    };
    let now = Local::now();
    let time = at.format("%-I:%M %p").to_string();
    if at.date_naive() == now.date_naive() {
        format!("Today at {time}")
    } else if at.date_naive() == now.date_naive().pred_opt().unwrap_or(now.date_naive()) {
        format!("Yesterday at {time}")
    } else if at.year() == now.year() {
        format!("{} at {time}", at.format("%b %-d"))
    } else {
        format!("{} at {time}", at.format("%b %-d, %Y"))
    }
}

/// A 36 pt round control: × or a chevron.
fn control(id: &'static str, glyph: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .size(px(CONTROL))
        .rounded_full()
        .bg(white(if enabled { 0.18 } else { 0.08 }))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |el| {
            el.cursor_pointer().hover(|s| s.bg(white(0.28)))
        })
        .child(icon(glyph, 14.0, white(if enabled { 1.0 } else { 0.35 })))
}

/// A bottom pill: glyph + label on white 16 %.
fn pill(id: &'static str, glyph: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h(px(PILL_HEIGHT))
        .px(px(14.0))
        .rounded_full()
        .bg(white(0.16))
        .flex()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .hover(|s| s.bg(white(0.26)))
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(white(1.0))
        .child(icon(glyph, 14.0, white(1.0)))
        .child(label)
}

/// The viewer, laid over the shell root while `state.media_viewer` is `Some`.
pub fn overlay(shell: &Shell, focus: &FocusHandle, cx: &mut Context<Shell>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let Some(viewer) = state.media_viewer.clone() else {
        return div().into_any_element();
    };
    let Some(item) = viewer.current().cloned() else {
        return div().into_any_element();
    };
    let count = viewer.items.len();
    let kind = item.attachment.kind();
    // A profile picture reads the avatar cache by URL; a chat picture its blob thumbnail.
    let is_profile_picture = item.picture_url.is_some();
    let picture = match &item.picture_url {
        Some(url) => state.avatar(url),
        None => item
            .attachment
            .blob_ref
            .as_ref()
            .and_then(|k| state.thumbnails.get(k).cloned()),
    };
    // The avatar cache has no failure state: a profile picture shows "Loading…" until it lands.
    let failed = !is_profile_picture
        && (item
            .attachment
            .blob_ref
            .as_ref()
            .is_some_and(|k| state.thumbnails_failed.contains(k))
            || item.attachment.blob_ref.is_none());
    let can_act = item
        .attachment
        .blob_ref
        .as_deref()
        .is_some_and(|k| !k.starts_with("local:"));
    let busy = viewer.busy.is_some();

    let close = shell.state.clone();
    let close_x = shell.state.clone();
    let close_pill = shell.state.clone();
    let close_scroll = shell.state.clone();
    let prev = shell.state.clone();
    let next = shell.state.clone();
    let act_prev = shell.state.clone();
    let act_next = shell.state.clone();
    let download = shell.state.clone();
    let forward = shell.state.clone();

    // ----- caption, counter, × -------------------------------------------
    let mut subline = if is_profile_picture {
        item.attachment.name.clone()
    } else {
        format!("{} · {}", when_label(item.when), item.attachment.name)
    };
    let size = item.attachment.size_label();
    if !size.is_empty() && !is_profile_picture {
        subline.push_str(" · ");
        subline.push_str(&size);
    }
    let caption = div()
        .absolute()
        .top(px(22.0))
        .left(px(CAPTION_LEFT))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(white(1.0))
                .child(item.by.clone()),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(white(0.67))
                .child(subline),
        );
    let counter = div()
        .absolute()
        .top(px(30.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .when(count > 1, |el| {
            el.child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(white(0.67))
                    .child(format!("{} of {count}", viewer.index + 1)),
            )
        });
    let close_btn = div().absolute().top(px(22.0)).right(px(EDGE)).child(
        control("media-viewer-close", "xmark", true).on_click(move |_, _, cx| {
            cx.stop_propagation();
            close_x.update(cx, |s, cx| s.close_media_viewer(cx));
        }),
    );

    // ----- the item --------------------------------------------------------
    let body: AnyElement = match kind {
        AttachmentKind::Image => match picture {
            Some(image) => img(image)
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None if failed => placeholder("photo", "No preview", &item, &theme).into_any_element(),
            None => div()
                .text_size(px(13.0))
                .text_color(white(0.67))
                .child("Loading…")
                .into_any_element(),
        },
        AttachmentKind::Video => {
            let play = shell.state.clone();
            div()
                .relative()
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w_full()
                        .h_full()
                        .rounded(px(8.0))
                        .bg(white(0.06))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("video", 44.0, white(0.25))),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(14.0))
                        .child(
                            div()
                                .id("media-viewer-play")
                                .size(px(76.0))
                                .rounded_full()
                                .bg(Theme::with_alpha(gpui::black(), 0.47))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(can_act && !busy, |el| {
                                    el.cursor_pointer()
                                        .hover(|s| s.bg(Theme::with_alpha(gpui::black(), 0.62)))
                                })
                                .child(icon("play", 30.0, white(1.0)))
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    play.update(cx, |s, cx| s.media_viewer_open_external(cx));
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(white(0.67))
                                .child(if busy {
                                    "Opening…"
                                } else {
                                    "Opens in QuickTime Player"
                                }),
                        ),
                )
                .into_any_element()
        }
        _ => file_body(&item, busy, can_act, shell.state.clone(), &theme).into_any_element(),
    };

    let mut root = div()
        .id("media-viewer")
        .debug_selector(|| "media-viewer".into())
        .key_context("MediaViewer")
        .track_focus(focus)
        .on_action(move |_: &Prev, _, cx| {
            act_prev.update(cx, |s, cx| s.media_viewer_step(-1, cx));
        })
        .on_action(move |_: &Next, _, cx| {
            act_next.update(cx, |s, cx| s.media_viewer_step(1, cx));
        })
        .absolute()
        .inset_0()
        .occlude()
        // Swipe / scroll down closes — the iPhone's dismiss gesture on a Mac
        // trackpad ([feedback app-01M5N9BJNZX8T1EQCX3RAB7GXD]).
        .on_scroll_wheel(move |event, _, cx| {
            cx.stop_propagation();
            let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
            if dy > DISMISS_SCROLL_PX {
                close_scroll.update(cx, |s, cx| s.close_media_viewer(cx));
            }
        })
        .bg(Theme::with_alpha(gpui::black(), BACKDROP))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            close.update(cx, |s, cx| s.close_media_viewer(cx));
        })
        .child(
            div()
                .absolute()
                .top(px(PICTURE_TOP))
                .bottom(px(PICTURE_BOTTOM))
                .left(px(EDGE + CONTROL + 20.0))
                .right(px(EDGE + CONTROL + 20.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("media-viewer-body")
                        .w_full()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(body),
                ),
        )
        .child(caption)
        .child(counter);

    if count > 1 {
        let at_start = viewer.index == 0;
        let at_end = viewer.index + 1 == count;
        root = root
            .child(
                div()
                    .absolute()
                    .left(px(EDGE))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        control("media-viewer-prev", "chevron-left", !at_start).on_click(
                            move |_, _, cx| {
                                cx.stop_propagation();
                                prev.update(cx, |s, cx| s.media_viewer_step(-1, cx));
                            },
                        ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right(px(EDGE))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        control("media-viewer-next", "chevron-right", !at_end).on_click(
                            move |_, _, cx| {
                                cx.stop_propagation();
                                next.update(cx, |s, cx| s.media_viewer_step(1, cx));
                            },
                        ),
                    ),
            );
    }

    // × after the chevrons so the full-height next strip cannot steal its hits
    // when the album has more than one item.
    root = root.child(close_btn);

    // ----- Close · Download · Forward (a profile picture: Close only) -----
    root = root.child(
        div()
            .absolute()
            .bottom(px(27.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .gap(px(10.0))
            .child(
                pill("media-viewer-close-pill", "xmark", "Close").on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    close_pill.update(cx, |s, cx| s.close_media_viewer(cx));
                }),
            )
            .when(!is_profile_picture, |el| {
                el.child(
                    pill("media-viewer-download", "arrow-down", "Download").on_click(
                        move |_, _, cx| {
                            cx.stop_propagation();
                            download.update(cx, |s, cx| s.media_viewer_download(cx));
                        },
                    ),
                )
                .child(
                    pill("media-viewer-forward", "forward", "Forward").on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        forward.update(cx, |s, cx| s.media_viewer_toggle_forward(cx));
                    }),
                )
            }),
    );

    if viewer.forward_open {
        root = root.child(forward_picker(
            shell,
            &item.by,
            ForwardSource::Media,
            &theme,
            cx,
        ));
    }
    root.into_any_element()
}

/// What the "Forward to" picker sends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardSource {
    /// The media viewer's open item.
    Media,
    /// The bubble the menu's Forward was clicked on (`AppState::forward_message`).
    Message,
}

/// The bubble menu's Forward: the same picker over the chat (its backdrop
/// dims the window; a click outside or Esc closes it).
pub fn message_forward_overlay(shell: &Shell, cx: &mut Context<Shell>) -> Option<AnyElement> {
    let by = shell.state.read(cx).forward_message.as_ref()?.by.clone();
    let theme = Theme::of(cx).clone();
    Some(forward_picker(shell, &by, ForwardSource::Message, &theme, cx).into_any_element())
}

/// "No preview" (image bytes missing) — the glyph and the file's name.
fn placeholder(glyph: &'static str, line: &str, item: &ViewerItem, _theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(12.0))
        .child(icon(glyph, 44.0, white(0.35)))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(white(0.67))
                .child(format!("{line} · {}", item.attachment.name)),
        )
}

/// A file / PDF / document: its tile, name, size · kind, and Quick Look.
fn file_body(
    item: &ViewerItem,
    busy: bool,
    can_act: bool,
    state: gpui::Entity<AppState>,
    theme: &Theme,
) -> Div {
    let ext = item
        .attachment
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_uppercase())
        .filter(|e| (2..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()));
    let tile = div()
        .w(px(120.0))
        .h(px(150.0))
        .rounded(px(14.0))
        .bg(white(0.11))
        .flex()
        .items_center()
        .justify_center();
    let tile = match ext {
        Some(e) => tile
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(white(1.0))
            .child(e),
        None => tile.child(icon("doc", 36.0, white(0.8))),
    };
    let mut meta = item.attachment.size_label();
    if !meta.is_empty() {
        meta.push_str(" · ");
    }
    meta.push_str(item.attachment.kind().label());
    let _ = theme;
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.0))
        .child(tile)
        .child(
            div()
                .pt(px(8.0))
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(white(1.0))
                .child(item.attachment.name.clone()),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(white(0.67))
                .child(meta),
        )
        .child(
            div().pt(px(14.0)).child(
                pill(
                    "media-viewer-quicklook",
                    "eye",
                    if busy { "Opening…" } else { "Quick Look" },
                )
                .when(!can_act || busy, |el| el.cursor_default())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    state.update(cx, |s, cx| s.media_viewer_open_external(cx));
                }),
            ),
        )
}

/// The Forward picker: a card with a search field and every DM, agent and
/// group, most recent first; one click sends the item (the viewer's media or
/// the menu's message, per `source`) there. `by` names the original sender.
fn forward_picker(
    shell: &Shell,
    by: &str,
    source: ForwardSource,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let query = shell.viewer_search.read(cx).text().to_string();
    let targets: Vec<ForwardTarget> = shell.state.read(cx).forward_targets(&query);
    let close = shell.state.clone();
    let close_x = shell.state.clone();
    let dismiss = move |s: &mut AppState, cx: &mut Context<AppState>| match source {
        ForwardSource::Media => s.media_viewer_toggle_forward(cx),
        ForwardSource::Message => s.close_forward_message(cx),
    };
    // The field is the sheet's standard search field: as tall as a list row's
    // avatar band, text the size of the row names (Grok's palette field reads
    // like its row titles), not the composer's body size.
    let search = div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .h(px(PICKER_FIELD_HEIGHT))
        .mx(px(12.0))
        .px(px(10.0))
        .rounded(px(8.0))
        .bg(theme.surface_2)
        .cursor_text()
        .text_size(px(PICKER_FIELD_FONT))
        .child(icon("search", 12.0, theme.text_muted))
        .child(div().flex_1().min_w_0().child(shell.viewer_search.clone()));
    let mut list = div()
        .id("media-viewer-forward-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(px(8.0))
        .py(px(6.0));
    if targets.is_empty() {
        list = list.child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(if query.trim().is_empty() {
                    "No chats yet.".to_string()
                } else {
                    format!("No chat matches “{}”.", query.trim())
                }),
        );
    }
    for (i, target) in targets.into_iter().enumerate() {
        let state = shell.state.read(cx);
        let avatar = state
            .project(&target.space_id)
            .map(|p| crate::ui::groups::picture(p, state, 36.0, theme))
            .unwrap_or_else(|| avatars::element(&target.space_id, &target.name, None, 36.0, theme));
        let pick = target.clone();
        list = list.child(
            div()
                .id(ElementId::NamedInteger(
                    "media-viewer-forward-row".into(),
                    i as u64,
                ))
                .h(px(PICKER_ROW))
                .px(px(8.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .child(avatar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(target.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme.text_muted)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(target.subtitle.clone()),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let pick = pick.clone();
                    this.state.update(cx, |s, cx| match source {
                        ForwardSource::Media => s.forward_media_to(pick, cx),
                        ForwardSource::Message => s.forward_message_to(pick, cx),
                    });
                })),
        );
    }
    let footnote = div()
        .px(px(16.0))
        .py(px(10.0))
        .border_t_1()
        .border_color(theme.border)
        .text_size(px(TEXT_TINY))
        .text_color(theme.text_muted)
        .child(format!("Sends it as a new message, “Forwarded from {by}”."));
    let card = div()
        .id("media-viewer-forward")
        .w(px(PICKER_WIDTH))
        .h(px(PICKER_HEIGHT))
        .rounded(px(14.0))
        .bg(theme.surface_1)
        .border_1()
        .border_color(theme.border_strong)
        .flex()
        .flex_col()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pl(px(16.0))
                .pr(px(12.0))
                .pt(px(12.0))
                .pb(px(10.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child("Forward to"),
                )
                .child(
                    div()
                        .id("media-viewer-forward-close")
                        .size(px(24.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_2))
                        .child(icon("xmark", 11.0, theme.text_muted))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            close_x.update(cx, |s, cx| dismiss(s, cx));
                        }),
                ),
        )
        .child(search)
        .child(list)
        .child(footnote);
    modal_overlay(
        "media-viewer-forward-backdrop",
        move |_, cx| close.update(cx, |s, cx| dismiss(s, cx)),
        card,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_label_names_today() {
        let now = crate::model::now_ms();
        assert!(when_label(now).starts_with("Today at "));
    }

    #[test]
    fn pill_and_control_sizes_match_the_mock() {
        assert_eq!(CONTROL, 36.0);
        assert_eq!(PILL_HEIGHT, 34.0);
    }
}
