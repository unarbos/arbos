//! Attachment cards (feedback chat-preview-cards-2026-10-23, Grok-black): a
//! **picture is its own bubble** — the trimmed image, aspect-fit, bubble radius,
//! a slim hairline so dark screenshots do not melt into the chat, and no footer
//! (a ↓ disc appears on hover; the viewer has Download and Forward). A **file**
//! is Grok's doc card (web-08): kind-coloured doc glyph, name, "PDF · 1.8 MB".
//! Clicking either opens the media viewer.

use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, AnyElement, App, Div, Entity, FontWeight, Hsla, ObjectFit,
    RenderImage, SharedString,
};

use crate::app::{is_https_page_url, AppState, FORWARDED_FROM_PREFIX};
use crate::model::{Attachment, AttachmentKind};
use crate::ui::highlight;
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, BUBBLE_RADIUS};

/// What a card click does: open the media viewer on this attachment.
pub type OpenFn = Rc<dyn Fn(&mut App)>;

/// Widest a picture bubble or a file card gets (the old card's width).
pub const CARD_WIDTH: f32 = 360.0;
/// Tallest a picture bubble gets; wider-than-tall pictures stop at `CARD_WIDTH`.
pub const PICTURE_MAX_HEIGHT: f32 = 420.0;
/// Narrowest a picture bubble gets (a tiny icon still reads as a bubble).
pub const PICTURE_MIN_WIDTH: f32 = 120.0;
/// The placeholder while the bytes load (or when there are none).
pub const PICTURE_PLACEHOLDER: (f32, f32) = (280.0, 180.0);
/// Grok's doc card (web-08): 61 pt on the iPhone's 17 pt text; 56 on the Mac's 13 pt.
pub const FILE_CARD_HEIGHT: f32 = 56.0;
pub const FILE_CARD_RADIUS: f32 = 14.0;
/// The doc glyph: 20 × 27 (Grok 21 × 27).
const DOC_W: f32 = 20.0;
const DOC_H: f32 = 27.0;
/// The hover ↓ disc.
const DISC: f32 = 28.0;
/// A picker sticker's square (ARB-544): no bubble, no hairline, fixed size.
pub const STICKER_SIZE: f32 = 128.0;

/// The doc glyph's colour by kind: Grok's Docs yellow for documents, PDF red,
/// Sheets green, Slides orange; media and the rest stay neutral.
pub fn kind_color(kind: AttachmentKind, theme: &Theme) -> Hsla {
    let rgb = |r: u8, g: u8, b: u8| gpui::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    };
    match kind {
        AttachmentKind::Pdf => rgb(0xE5, 0x3E, 0x3E).into(),
        AttachmentKind::Doc => rgb(0xFF, 0xB0, 0x00).into(),
        AttachmentKind::Sheet => rgb(0x2E, 0xB8, 0x6A).into(),
        AttachmentKind::Slides => rgb(0xF2, 0x8B, 0x2E).into(),
        AttachmentKind::Image
        | AttachmentKind::Video
        | AttachmentKind::Audio
        | AttachmentKind::Sticker
        | AttachmentKind::File => theme.text_muted,
    }
}

/// What the picture bubble shows.
pub enum Preview {
    /// The thumbnail bytes are on their way.
    Loading,
    Ready(Arc<RenderImage>),
    /// No blob, or the fetch failed: the file is still listed and downloadable.
    Unavailable,
}

/// The size a picture bubble takes for an image of `w × h` device pixels:
/// aspect-fit inside `CARD_WIDTH × PICTURE_MAX_HEIGHT`, never narrower than
/// `PICTURE_MIN_WIDTH` (a tall sliver keeps its ratio inside that width).
pub fn picture_size(w: f32, h: f32) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 {
        return PICTURE_PLACEHOLDER;
    }
    // Fill the bubble width (Grok's pictures run to the bubble's width) unless
    // the picture is tall — then its height caps it and the width follows.
    let scale = (CARD_WIDTH / w).min(PICTURE_MAX_HEIGHT / h);
    let mut dw = (w * scale).min(CARD_WIDTH);
    let mut dh = (h * scale).min(PICTURE_MAX_HEIGHT);
    if dw < PICTURE_MIN_WIDTH {
        let s = PICTURE_MIN_WIDTH / dw;
        dw = PICTURE_MIN_WIDTH;
        dh = (dh * s).min(PICTURE_MAX_HEIGHT);
    }
    (dw.round(), dh.round())
}

/// The ↓ disc: 28 pt, black 55 %, white glyph; the caller shows it on hover.
fn download_disc(id: SharedString, muted: bool, theme: &Theme) -> gpui::Stateful<Div> {
    let (bg, fg) = if muted {
        (theme.surface_2, theme.text_muted)
    } else {
        (Theme::with_alpha(gpui::black(), 0.55), gpui::white())
    };
    div()
        .id(id)
        .size(px(DISC))
        .rounded_full()
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        // hover-fill: exception overlay
        .hover(|s| s.bg(Theme::with_alpha(gpui::black(), 0.75)))
        .child(sf_symbols::glyph("arrow.down", 12.0, fg))
}

/// The card element for one attachment row. `forwarded_from` is the message's
/// "Forwarded from …" line, shown as the card's hint instead of a bubble.
/// `quote_header` is a reply's quote, drawn above the picture / card like a text
/// bubble's ([feedback picture-reply-quote-mac-2026-10-28]).
#[allow(clippy::too_many_arguments)]
pub fn card(
    row_id: &str,
    attachment: &Attachment,
    preview: Preview,
    forwarded_from: Option<&str>,
    quote_header: Option<AnyElement>,
    open: Option<OpenFn>,
    state: Entity<AppState>,
    theme: &Theme,
) -> AnyElement {
    if attachment.is_sticker() {
        return sticker_tile(row_id, preview, open, theme);
    }
    if attachment.kind() == AttachmentKind::Image {
        picture_bubble(
            row_id,
            attachment,
            preview,
            forwarded_from,
            quote_header,
            open,
            state,
            theme,
        )
    } else {
        file_card(
            row_id,
            attachment,
            forwarded_from,
            quote_header,
            open,
            state,
            theme,
        )
    }
}

/// A sticker (ARB-544): the picture alone on the chat background at a fixed square —
/// no bubble, no hairline, no caption — like Telegram / iMessage stickers. Click opens the
/// viewer like any picture. Both apps draw this the same (iPhone 160 pt, Mac 128 pt).
fn sticker_tile(row_id: &str, preview: Preview, open: Option<OpenFn>, theme: &Theme) -> AnyElement {
    let mut tile = div()
        .id(SharedString::from(row_id.to_owned()))
        .w(px(STICKER_SIZE))
        .h(px(STICKER_SIZE))
        .flex()
        .items_center()
        .justify_center();
    tile = match preview {
        // Keyed so an animated sticker plays; see the note in `picture_bubble`.
        Preview::Ready(image) => tile.child(
            img(image)
                .id(SharedString::from(format!("{row_id}#sticker")))
                .w(px(STICKER_SIZE))
                .h(px(STICKER_SIZE))
                .object_fit(ObjectFit::Contain),
        ),
        Preview::Loading => tile.child(
            div()
                .size(px(STICKER_SIZE * 0.6))
                .rounded(px(BUBBLE_RADIUS))
                .bg(theme.surface_2),
        ),
        Preview::Unavailable => tile.child(sf_symbols::glyph("photo", 22.0, theme.text_faint)),
    };
    if let Some(open) = open {
        tile = tile.cursor_pointer().on_click(move |_, _, cx| open(cx));
    }
    tile.into_any_element()
}

/// A picture: aspect-fit at its own ratio, bubble radius, slim outline. Hover
/// shows the ↓ disc top-right; the whole bubble opens the viewer.
#[allow(clippy::too_many_arguments)]
fn picture_bubble(
    row_id: &str,
    attachment: &Attachment,
    preview: Preview,
    forwarded_from: Option<&str>,
    quote_header: Option<AnyElement>,
    open: Option<OpenFn>,
    state: Entity<AppState>,
    theme: &Theme,
) -> AnyElement {
    // `preview_key`, not `ref`: a picked GIF's bytes come through the relay's pass-through, so
    // it had no Save disc ([feedback gif-send-and-playback-2026-10-30]).
    let key = attachment.preview_key();
    let name = attachment.name.clone();
    let group = SharedString::from(format!("{row_id}-picture"));
    let (w, h) = match &preview {
        Preview::Ready(image) => {
            let size = image.size(0);
            picture_size(size.width.0 as f32, size.height.0 as f32)
        }
        _ => PICTURE_PLACEHOLDER,
    };
    let mut bubble = div()
        .id(SharedString::from(row_id.to_owned()))
        .group(group.clone())
        .relative()
        .w(px(w))
        .h(px(h))
        .max_w_full()
        .rounded(px(BUBBLE_RADIUS))
        .overflow_hidden()
        // Thin hairline so dark screenshots stay readable on the chat bg
        // (same idea as the group-chat left-edge accent, neutral colour).
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.bubble_assistant)
        .flex()
        .items_center()
        .justify_center();
    bubble = match preview {
        // The id is what makes an animated GIF play: GPUI keeps the frame cursor in element
        // state and asks for the next animation frame only for an `img` it can key, so
        // without one every GIF sat on frame 0 for ever
        // ([feedback gif-send-and-playback-2026-10-30]).
        Preview::Ready(image) => bubble.child(
            img(image)
                .id(SharedString::from(format!("{row_id}#picture")))
                .w(px(w))
                .h(px(h))
                .rounded(px(BUBBLE_RADIUS))
                .object_fit(ObjectFit::Contain),
        ),
        Preview::Loading => bubble.child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_faint)
                .child("Loading…"),
        ),
        Preview::Unavailable => bubble.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(sf_symbols::glyph("photo", 22.0, theme.text_faint))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_faint)
                        .child(name.clone()),
                ),
        ),
    };
    if let Some(open) = open {
        bubble = bubble.cursor_pointer().on_click(move |_, _, cx| open(cx));
    }
    // The ↓ disc fades in with the hover; a click saves without opening the viewer.
    if let Some(key) = key.filter(|k| !k.starts_with("local:")) {
        let state = state.clone();
        bubble = bubble.child(
            div()
                .absolute()
                .top(px(8.0))
                .right(px(8.0))
                .opacity(0.0)
                .group_hover(group, |s| s.opacity(1.0))
                .child(
                    download_disc(
                        SharedString::from(format!("{row_id}-download")),
                        false,
                        theme,
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let (key, name) = (key.clone(), name.clone());
                        state.update(cx, |s, cx| s.download_attachment(key, name, cx));
                    }),
                ),
        );
    }
    let mut out = div().flex().flex_col().gap(px(4.0));
    if let Some(from) = forwarded_from {
        // The same header a forwarded text bubble carries, above the picture
        // (Telegram / iMessage) — so a forwarded picture + caption reads as one
        // forwarded message ([feedback forward-attachments-mac-2026-10-27]).
        let by = from.strip_prefix(FORWARDED_FROM_PREFIX).unwrap_or(from);
        out = out.child(div().pl(px(6.0)).max_w(px(w)).child(
            crate::ui::transcript::forwarded_header(by.to_owned(), theme.text),
        ));
    }
    if let Some(quote) = quote_header {
        // A reply's quote above the picture, where a text bubble carries it
        // ([feedback picture-reply-quote-mac-2026-10-28]).
        out = out.child(div().pl(px(6.0)).max_w(px(w)).child(quote));
    }
    out = out.child(bubble);
    // GIF-picker attribution (feedback app-01M5TGH18WV3QCK5KZT6XKG7FG /
    // app-01M61EZD0GJ2XGV1EW4SCVXENK): a small "via <provider>" under the
    // picture. Only an https `page_url` is a link; other schemes stay plain.
    if let Some(src) = attachment.source.clone() {
        let label = format!("via {}", src.provider);
        let url = src.page_url.clone();
        let mut via = div()
            .id(SharedString::from(format!("{row_id}-via")))
            .pl(px(6.0))
            .max_w(px(w))
            .text_size(px(11.0))
            .text_color(theme.text_faint)
            .child(label);
        if is_https_page_url(&url) {
            via = via.cursor_pointer().on_click(move |_, _, cx| {
                cx.open_url(&url);
            });
        }
        out = out.child(via);
    }
    out.into_any_element()
}

/// Grok's doc card (web-08): kind-coloured doc glyph, name, "PDF · 1.8 MB";
/// no Download pill — the ↓ disc appears on hover, the viewer has the rest.
#[allow(clippy::too_many_arguments)]
fn file_card(
    row_id: &str,
    attachment: &Attachment,
    forwarded_from: Option<&str>,
    quote_header: Option<AnyElement>,
    open: Option<OpenFn>,
    state: Entity<AppState>,
    theme: &Theme,
) -> AnyElement {
    let kind = attachment.kind();
    let key = attachment.blob_ref.clone();
    let name = attachment.name.clone();
    let group = SharedString::from(format!("{row_id}-file"));
    let mut subtitle = kind.label().to_owned();
    let size = attachment.size_label();
    if !size.is_empty() {
        subtitle.push_str(" · ");
        subtitle.push_str(&size);
    }
    if let Some(from) = forwarded_from {
        subtitle = format!("{from} · {subtitle}");
    }
    let glyph = div()
        .flex_shrink_0()
        .w(px(DOC_W))
        .h(px(DOC_H))
        .rounded(px(5.0))
        .bg(kind_color(kind, theme))
        .flex()
        .items_center()
        .justify_center()
        .child(sf_symbols::glyph(
            match kind {
                AttachmentKind::Video => "play.fill",
                AttachmentKind::Audio => "waveform",
                _ => "doc",
            },
            11.0,
            gpui::black(),
        ));
    let mut card = div()
        .id(SharedString::from(row_id.to_owned()))
        .group(group.clone())
        .w(px(CARD_WIDTH))
        .max_w_full()
        .h(px(FILE_CARD_HEIGHT))
        .rounded(px(FILE_CARD_RADIUS))
        .bg(theme.bubble_assistant)
        .flex()
        .items_center()
        .gap(px(14.0))
        .pl(px(15.0))
        .pr(px(12.0))
        .child(glyph)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(attachment.title().to_owned()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(subtitle),
                ),
        );
    if let Some(key) = key.filter(|k| !k.starts_with("local:")) {
        let state = state.clone();
        card = card.child(
            div()
                .flex_shrink_0()
                .opacity(0.0)
                .group_hover(group, |s| s.opacity(1.0))
                .child(
                    download_disc(
                        SharedString::from(format!("{row_id}-download")),
                        true,
                        theme,
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let (key, name) = (key.clone(), name.clone());
                        state.update(cx, |s, cx| s.download_attachment(key, name, cx));
                    }),
                ),
        );
    }
    if let Some(open) = open {
        card = highlight::button(card.cursor_pointer(), theme).on_click(move |_, _, cx| open(cx));
    }
    match quote_header {
        // A reply's quote above the card, where a text bubble carries it
        // ([feedback picture-reply-quote-mac-2026-10-28]).
        Some(quote) => div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(div().pl(px(6.0)).max_w(px(CARD_WIDTH)).child(quote))
            .child(card)
            .into_any_element(),
        None => card.into_any_element(),
    }
}

/// The "Forwarded from <name>" line of a message, when its text is one.
pub fn forwarded_from(text: &str) -> Option<&str> {
    let t = text.trim();
    (t.starts_with(FORWARDED_FROM_PREFIX) && !t.contains('\n')).then_some(t)
}

/// Reveal the saved file: Finder on macOS, the file manager (or the default app) elsewhere.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let spawned = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    let spawned = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(err) = spawned {
        eprintln!("reveal {}: {err}", path.display());
    }
}

/// Hand a fetched file to macOS: Quick Look (`qlmanage -p`) for documents, the
/// default app for the rest. Videos in the media viewer play inline instead
/// ([feedback app-01M6147S9TPNBQA7KSDEY0K8C4]); this path is for Quick Look
/// and non-mac fallbacks. Elsewhere `xdg-open`.
pub fn open_with_system(path: &Path, quick_look: bool) {
    #[cfg(target_os = "macos")]
    let spawned = if quick_look {
        Command::new("qlmanage")
            .arg("-p")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        Command::new("open").arg(path).spawn()
    };
    #[cfg(not(target_os = "macos"))]
    let spawned = {
        let _ = quick_look;
        Command::new("xdg-open")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    if let Err(err) = spawned {
        eprintln!("open {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_the_bubble_at_their_own_ratio() {
        // Wide: full width, height follows.
        assert_eq!(picture_size(1200.0, 750.0), (360.0, 225.0));
        // Tall: height caps it, width follows.
        assert_eq!(picture_size(750.0, 1200.0), (263.0, 420.0));
        // A sliver keeps the minimum width.
        let (w, h) = picture_size(100.0, 1000.0);
        assert_eq!(w, PICTURE_MIN_WIDTH);
        assert!(h <= PICTURE_MAX_HEIGHT);
        // Unknown size → the placeholder.
        assert_eq!(picture_size(0.0, 0.0), PICTURE_PLACEHOLDER);
    }

    #[test]
    fn file_card_matches_groks_doc_card() {
        assert_eq!(FILE_CARD_HEIGHT, 56.0);
        assert_eq!((DOC_W, DOC_H), (20.0, 27.0));
    }
}
