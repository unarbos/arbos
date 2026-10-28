//! The one avatar + name layout (Jacob, 2026-10-22: "names need to be pulled
//! back in"): a fixed picture slot, one gap, then the text, which truncates.
//! The header pill (every chat kind), the sidebar rows and the kit's people /
//! agent / group rows all draw through [`row`], so the picture can never be
//! squeezed under a long name and the gap is measured in exactly one place.
//!
//! Gaps are Grok Bot's, measured on the real Mac app (ink to ink, 2 px/pt):
//! the chat title's blob → name is 5 pt (ref 57); a sidebar / members row's
//! blob → name is 11–12 pt (refs 45, 39). Measured on our builds, the 13 pt
//! semibold header name starts flush with its box and the 15 pt row name
//! about 1 pt in, so the box gaps below land on Grok's ink gaps.
//!
//! Names that are only emoji ([feedback emoji-name-handle-offset-mac-2026-10-28])
//! are drawn through [`name`]: the emoji cell is lifted onto the text
//! baseline so a `@handle` beside or under the name keeps the relation it has
//! with a text name — see [`EMOJI_CELL_DESCENT_EM`].

use gpui::{div, prelude::*, px, AnyElement, Div, SharedString};
use unicode_segmentation::UnicodeSegmentation;

/// Picture → name in the chat header pill (Grok: 5 pt ink).
pub const HEADER_GAP: f32 = 5.0;
/// Picture → name in a list row — sidebar, members, people (Grok: 11–12 pt ink).
pub const ROW_GAP: f32 = 10.0;

/// A fixed `size` square for the picture. Whatever the picture does — a leaf's
/// hover gust, a photo, a stacked badge hanging off the corner — the slot keeps
/// its width, so the name's left edge is `slot right + gap` at every width.
pub fn slot(size: f32, picture: impl IntoElement) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(|| "identity-picture".into())
        .child(picture)
}

/// How far Apple Color Emoji hangs under the text baseline, in em. The
/// `.AppleColorEmojiUI` face the system font falls back to draws every glyph
/// in one cell: `CTRunGetImageBounds` of a 15 pt emoji run is x 1.07, y −3.75,
/// 18.75 × 18.75 — the cell spans −0.25 em … +1.0 em, where SF's capitals end
/// at 0.70 em (`CTFontGetCapHeight`) and its lowercase at 0.51. GPUI's line
/// box never changes with the glyphs (`line_height × lines`, and the mac text
/// system's ascent is the requested font's), so an emoji name's *box* is a
/// text name's box: the handle beside it (`items_baseline`, bottom-aligned
/// boxes) or under it does not move. What moves is the ink: the emoji's
/// bottom lands 0.25 em under the baseline the handle sits on, so the
/// handle reads as hanging mid-emoji ("the handle is displayed on a different
/// height when the name is an emoji", Scout, 2026-10-28). [`name_lift`]
/// raises an emoji-only name by this much, so its cell rests on the baseline
/// like a capital — the same bottom edge the handle has against text.
///
/// Mixed names ("Cato 🌳") are left as Core Text sets them: the handle aligns
/// with the text's baseline and the emoji hangs as it does in any Mac app.
pub const EMOJI_CELL_DESCENT_EM: f32 = 0.25;

/// `true` when every grapheme of `name` is an emoji (skin tones, ZWJ
/// sequences, flags, keycaps and VS16 included) — spaces between them allowed,
/// nothing else. Letters, digits (outside a keycap) or punctuation anywhere
/// make it a text or mixed name.
pub fn emoji_only(name: &str) -> bool {
    let mut saw_emoji = false;
    for g in name.graphemes(true) {
        if g.chars().all(char::is_whitespace) {
            continue;
        }
        if !emoji_grapheme(g) {
            return false;
        }
        saw_emoji = true;
    }
    saw_emoji
}

/// One grapheme cluster that renders as a single emoji.
fn emoji_grapheme(g: &str) -> bool {
    let keycap = g.contains('\u{20E3}');
    let mut pictographic = false;
    for c in g.chars() {
        let u = c as u32;
        let is_joiner = matches!(u, 0x200D | 0xFE0E | 0xFE0F | 0x20E3) // ZWJ, variation selectors, keycap
            || (0x1F3FB..=0x1F3FF).contains(&u) // skin tones
            || (0xE0020..=0xE007F).contains(&u); // tag sequences (subdivision flags)
        let is_keycap_base = keycap && (c.is_ascii_digit() || c == '#' || c == '*');
        if is_pictographic(u) {
            pictographic = true;
        } else if !(is_joiner || is_keycap_base) {
            return false;
        }
    }
    pictographic || keycap
}

/// A scalar that is an emoji on its own (Unicode's `Emoji` property, the
/// pictographic part; modifiers and joiners are handled by the caller).
fn is_pictographic(u: u32) -> bool {
    matches!(
        u,
        0x1F000..=0x1FAFF // pictographs, emoticons, transport, flags, supplemental
            | 0x2600..=0x27BF // misc symbols, dingbats (☀ ✨ ❤)
            | 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139
            | 0x2194..=0x2199 | 0x21A9 | 0x21AA // the emoji arrows
            | 0x231A | 0x231B | 0x2328 | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA
            | 0x24C2 | 0x25AA | 0x25AB | 0x25B6 | 0x25C0 | 0x25FB..=0x25FE
            | 0x2934 | 0x2935 | 0x2B05..=0x2B07 | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55
            | 0x3030 | 0x303D | 0x3297 | 0x3299
    )
}

/// How far (pt) a name drawn at `size` is raised so an emoji cell rests on
/// the text baseline: `size × EMOJI_CELL_DESCENT_EM` for an emoji-only name,
/// 0 for text and mixed names.
pub fn name_lift(name: &str, size: f32) -> f32 {
    if emoji_only(name) {
        size * EMOJI_CELL_DESCENT_EM
    } else {
        0.0
    }
}

/// The debug selector a lifted name carries, so a headless render can check
/// which names were raised and by how much.
pub const LIFTED_SELECTOR: &str = "emoji-name-lifted";

/// A one-line name at `size`, truncating, that an emoji-only value lifts by
/// [`name_lift`] (a relative offset: the box, the row and whatever sits beside
/// or under it stay where a text name puts them). Every row, hero and sender
/// line that shows a name next to a handle or over a second line draws its
/// name through this.
pub fn name(text: impl Into<SharedString>, size: f32) -> Div {
    let text = text.into();
    let lift = name_lift(&text, size);
    div()
        .truncate()
        .text_size(px(size))
        .when(lift > 0.0, |el| {
            el.relative()
                .top(px(-lift))
                .debug_selector(|| LIFTED_SELECTOR.into())
        })
        .child(text)
}

/// Picture slot, `gap`, then `text` (which shrinks and truncates; the slot
/// never does). The caller adds id, hover, padding and trailing children.
pub fn row(size: f32, gap: f32, picture: impl IntoElement, text: AnyElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(gap))
        .min_w_0()
        .child(slot(size, picture))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .debug_selector(|| "identity-text".into())
                .child(text),
        )
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, Pixels, TestAppContext};

    use super::*;
    use crate::ui::avatars;

    /// Three picture kinds: a leaf disc (the hover kind, as in the header and
    /// rows), an initials disc, and a photo-shaped box with a badge hanging off
    /// its corner (the shared-project row).
    #[derive(Clone, Copy)]
    enum Picture {
        Leaf,
        Initials,
        PhotoWithBadge,
    }

    struct Row {
        width: f32,
        size: f32,
        gap: f32,
        picture: Picture,
        name: &'static str,
        subtitle: Option<&'static str>,
    }

    impl gpui::Render for Row {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let size = self.size;
            let picture: AnyElement = match self.picture {
                Picture::Leaf => {
                    avatars::leaf_element(avatars::LeafPick { leaf: 3, color: 4 }, size)
                        .into_any_element()
                }
                Picture::Initials => div()
                    .size(px(size))
                    .flex_shrink_0()
                    .rounded_full()
                    .child("LI")
                    .into_any_element(),
                Picture::PhotoWithBadge => div()
                    .relative()
                    .size(px(size))
                    .flex_shrink_0()
                    .child(
                        div()
                            .absolute()
                            .right(px(-4.0))
                            .bottom(px(-2.0))
                            .size(px(16.0)),
                    )
                    .into_any_element(),
            };
            let mut text = div().flex().flex_col().min_w_0();
            text = text.child(div().truncate().child(self.name));
            if let Some(sub) = self.subtitle {
                text = text.child(div().truncate().child(sub));
            }
            div()
                .w(px(self.width))
                .h(px(60.0))
                .flex()
                .child(row(size, self.gap, picture, text.into_any_element()).pr(px(4.0)))
        }
    }

    fn bounds(cx: &mut TestAppContext, r: Row) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let (_view, vcx) = cx.add_window_view(|_, _| r);
        vcx.run_until_parked();
        (
            vcx.debug_bounds("identity-picture").expect("picture drawn"),
            vcx.debug_bounds("identity-text").expect("text drawn"),
        )
    }

    const LONG: &str = "Lium Lium Lium Lium Lium Lium Lium Lium Lium Lium Lium Lium";
    const LONG_SUB: &str =
        "Arbos, two agents and a very long subtitle that never ends and keeps going";

    #[gpui::test]
    fn the_name_starts_at_the_slot_edge_plus_the_gap_at_every_width(cx: &mut TestAppContext) {
        for picture in [Picture::Leaf, Picture::Initials, Picture::PhotoWithBadge] {
            for (size, gap) in [(22.0, HEADER_GAP), (36.0, ROW_GAP)] {
                for width in [60.0, 120.0, 200.0, 320.0, 600.0] {
                    for (name, subtitle) in [("Lium", None), (LONG, Some(LONG_SUB))] {
                        let (pic, text) = bounds(
                            cx,
                            Row {
                                width,
                                size,
                                gap,
                                picture,
                                name,
                                subtitle,
                            },
                        );
                        let pic_w: f32 = pic.size.width.into();
                        let pic_right: f32 = pic.right().into();
                        let text_x: f32 = text.origin.x.into();
                        assert_eq!(pic_w, size, "slot keeps its size at width {width}");
                        assert!(
                            (text_x - (pic_right + gap)).abs() <= 0.5,
                            "name x {text_x} vs slot right {pic_right} + gap {gap} at width {width} ({name:?})"
                        );
                    }
                }
            }
        }
    }

    // ----- emoji names ([feedback emoji-name-handle-offset-mac-2026-10-28]) ---------

    const TEXT_NAME: &str = "Jacob";
    const EMOJI_NAME: &str = "🧙🏽\u{200d}♂\u{fe0f}";
    const MIXED_NAME: &str = "Cato 🌳";

    #[test]
    fn emoji_only_knows_emoji_from_text() {
        for emoji in [
            EMOJI_NAME,
            "🌳",
            "🌳🌲",
            "🙂 🙃",
            "👍🏽",
            "🇩🇪",
            "1️⃣",
            "❤️",
            "⭐️",
            "✨",
            "👨\u{200d}👩\u{200d}👧",
            "🏴\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}",
        ] {
            assert!(emoji_only(emoji), "{emoji:?} is an emoji-only name");
        }
        for text in [
            TEXT_NAME, MIXED_NAME, "", " ", "@mage", "🌳!", "x🌳", "→", "1", "#", "✓ done",
        ] {
            assert!(!emoji_only(text), "{text:?} is not an emoji-only name");
        }
    }

    #[test]
    fn the_lift_is_the_emoji_cells_descent_and_only_for_emoji_names() {
        assert_eq!(name_lift(EMOJI_NAME, 15.0), 15.0 * EMOJI_CELL_DESCENT_EM);
        assert_eq!(name_lift(EMOJI_NAME, 21.0), 21.0 * EMOJI_CELL_DESCENT_EM);
        assert_eq!(name_lift(TEXT_NAME, 15.0), 0.0);
        assert_eq!(name_lift(MIXED_NAME, 15.0), 0.0);
        // Apple Color Emoji's cell: −0.25 em … +1.0 em (`CTRunGetImageBounds`,
        // 15 pt: y −3.75, height 18.75).
        assert_eq!(EMOJI_CELL_DESCENT_EM, 0.25);
    }

    /// A name beside its `@handle` (the sidebar's DM row: `items_baseline`)
    /// and a name over it (a member row, the hero).
    struct Pair {
        name: &'static str,
        beside: bool,
    }

    impl gpui::Render for Pair {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            // The slot is the name's layout box; a lifted name is drawn above it.
            let name_el = div()
                .min_w_0()
                .debug_selector(|| "pair-name".into())
                .child(name(self.name, 15.0));
            let handle = div()
                .flex_shrink_0()
                .text_size(px(11.5))
                .debug_selector(|| "pair-handle".into())
                .child("@dawizzaa");
            let line = if self.beside {
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(name_el)
                    .child(handle)
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(name_el)
                    .child(handle)
            };
            div()
                .w(px(320.0))
                .h(px(80.0))
                .flex()
                .items_center()
                .child(line.debug_selector(|| "pair-line".into()))
        }
    }

    struct Laid {
        /// The name's layout box (unchanged by a lift).
        name: Bounds<Pixels>,
        handle: Bounds<Pixels>,
        line: Bounds<Pixels>,
        /// Where a lifted name was drawn, if it was lifted.
        lifted: Option<Bounds<Pixels>>,
    }

    fn lay(cx: &mut TestAppContext, pair: Pair) -> Laid {
        let (_view, vcx) = cx.add_window_view(|_, _| pair);
        vcx.run_until_parked();
        Laid {
            name: vcx.debug_bounds("pair-name").expect("name drawn"),
            handle: vcx.debug_bounds("pair-handle").expect("handle drawn"),
            line: vcx.debug_bounds("pair-line").expect("line drawn"),
            lifted: vcx.debug_bounds(LIFTED_SELECTOR),
        }
    }

    #[gpui::test]
    fn an_emoji_name_never_moves_the_handle_beside_or_under_it(cx: &mut TestAppContext) {
        for beside in [true, false] {
            let text = lay(
                cx,
                Pair {
                    name: TEXT_NAME,
                    beside,
                },
            );
            let emoji = lay(
                cx,
                Pair {
                    name: EMOJI_NAME,
                    beside,
                },
            );
            let mixed = lay(
                cx,
                Pair {
                    name: MIXED_NAME,
                    beside,
                },
            );
            let where_ = if beside { "beside" } else { "under" };
            // The handle and the line's box are where the text name puts them
            // (beside, its x follows the name's width; its height and top do not).
            let vertical = |b: Bounds<Pixels>| (b.origin.y, b.size.height);
            assert_eq!(
                vertical(emoji.handle),
                vertical(text.handle),
                "the handle {where_} an emoji name sits where it sits for text"
            );
            assert_eq!(
                vertical(mixed.handle),
                vertical(text.handle),
                "the handle {where_} a mixed name sits where it sits for text"
            );
            if !beside {
                assert_eq!(
                    emoji.handle.origin.x, text.handle.origin.x,
                    "under: same column"
                );
            }
            assert_eq!(
                emoji.line.size.height, text.line.size.height,
                "the line keeps its height ({where_})"
            );
            assert_eq!(
                mixed.line.size.height, text.line.size.height,
                "the line keeps its height ({where_})"
            );
            // The emoji name's layout box is the text name's box; its ink is
            // drawn raised by the cell's descent. The mixed name is left where
            // Core Text sets it.
            let lift = name_lift(EMOJI_NAME, 15.0);
            assert_eq!(
                vertical(emoji.name),
                vertical(text.name),
                "the emoji name's box is a text name's box ({where_})"
            );
            assert_eq!(
                vertical(mixed.name),
                vertical(text.name),
                "the mixed name's box is a text name's box ({where_})"
            );
            let drawn = emoji.lifted.expect("the emoji name is lifted");
            assert_eq!(
                drawn.size.height, text.name.size.height,
                "same line box ({where_})"
            );
            // Taffy rounds positions to whole pixels, so allow half a pixel.
            assert!(
                (f32::from(text.name.top() - drawn.top()) - lift).abs() <= 0.5,
                "emoji name raised by {lift}: box top {:?}, drawn top {:?} ({where_})",
                text.name.top(),
                drawn.top()
            );
            assert!(
                text.lifted.is_none() && mixed.lifted.is_none(),
                "text and mixed names are not lifted ({where_})"
            );
        }
    }

    #[test]
    fn the_gaps_are_grok_bots() {
        let (header, row) = (
            std::hint::black_box(HEADER_GAP),
            std::hint::black_box(ROW_GAP),
        );
        // Ref 57: 5 pt ink (no bearing at 13 pt semibold); refs 45 / 39:
        // 11–12 pt ink with ≈ 1 pt of bearing at 15 pt.
        assert!((4.5..=5.5).contains(&header), "header gap {header}");
        assert!((10.0..=11.0).contains(&row), "row gap {row}");
    }
}
