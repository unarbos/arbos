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

use gpui::{div, prelude::*, px, AnyElement, Div, SharedString};

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

/// A one-line name at `size`, truncating. Every row, hero and sender line
/// that shows a name next to a handle or over a second line draws it through
/// this.
///
/// Emoji-only names get no vertical offset: Apple Color Emoji's cell spans
/// −0.25 … +1.0 em around the baseline (`CTRunGetImageBounds`, 15 pt: y −3.75,
/// h 18.75), so Core Text already centres it on SF's capitals (0 … 0.70 em)
/// and the name sits level with a text name. Raising the cell onto the
/// baseline put it 0.25 em above one ([feedback emoji-name-baseline-2026-10-29]).
pub fn name(text: impl Into<SharedString>, size: f32) -> Div {
    div().truncate().text_size(px(size)).child(text.into())
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

    // ----- emoji names ([feedback emoji-name-baseline-2026-10-29]) ---------

    const TEXT_NAME: &str = "Jacob";
    const EMOJI_NAME: &str = "🧙🏽\u{200d}♂\u{fe0f}";
    const MIXED_NAME: &str = "Cato 🌳";

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
            let name_el = div()
                .min_w_0()
                .debug_selector(|| "pair-name".into())
                .child(name(self.name, 15.0).debug_selector(|| "pair-name-text".into()));
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
        name: Bounds<Pixels>,
        drawn: Bounds<Pixels>,
        handle: Bounds<Pixels>,
        line: Bounds<Pixels>,
    }

    fn lay(cx: &mut TestAppContext, pair: Pair) -> Laid {
        let (_view, vcx) = cx.add_window_view(|_, _| pair);
        vcx.run_until_parked();
        Laid {
            name: vcx.debug_bounds("pair-name").expect("name laid out"),
            drawn: vcx.debug_bounds("pair-name-text").expect("name drawn"),
            handle: vcx.debug_bounds("pair-handle").expect("handle drawn"),
            line: vcx.debug_bounds("pair-line").expect("line drawn"),
        }
    }

    #[gpui::test]
    fn an_emoji_name_sits_where_a_text_name_sits_beside_or_over_its_handle(
        cx: &mut TestAppContext,
    ) {
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
            // Every name is drawn in its own layout box, with no offset: Core
            // Text centres the emoji cell on the capitals.
            for (kind, laid) in [("text", &text), ("emoji", &emoji), ("mixed", &mixed)] {
                assert_eq!(
                    vertical(laid.drawn),
                    vertical(laid.name),
                    "the {kind} name is drawn in its box ({where_})"
                );
            }
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
