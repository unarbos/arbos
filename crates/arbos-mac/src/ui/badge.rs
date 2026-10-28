//! The one status badge on a picture: the machine dot under my footer picture
//! and under the agent in the chat header's pill, the rail's typing pulse and
//! unread dot, the agent mini-picture and the guest stack on a sidebar row.
//! Every picture-with-badge on the Mac is drawn by [`badged`] or [`corner`],
//! so the badge sits **in front of** the picture everywhere, with a ring in
//! the colour of the surface the picture rests on (the macOS / iOS
//! presence-badge shape: ring = row colour, 2 pt, dot sized to the picture).
//!
//! Why a component ([feedback presence-dot-layering-mac-2026-10-27], Jacob
//! 2026-10-27, dark, a sidebar picture's green dot "behind" the photo with only
//! a crescent of green past the circle): the sidebar, the rail and the header
//! pill paint through `frost::frosted`, which wraps its whole child in one
//! GPUI `paint_layer` (the sidebar and rail on Glass, the header pill on both
//! surfaces). Inside a layer every primitive shares one draw order, and the
//! renderer batches equal orders by kind — quads first, then paths, text and
//! icons, then photos (polychrome sprites). A dot drawn as a sibling *after* a
//! photo therefore still landed under it; only the part hanging past the
//! photo's circle showed. [`Above`] paints the badge in a layer of its own:
//! the scene's bounds tree gives that layer an order above everything already
//! painted under its bounds, so the badge is drawn after the picture whatever
//! kind of primitive the picture is, inside a frosted panel or not.
//! `ordering_tests` pins the renderer rule this relies on against the pinned
//! GPUI revision.

use gpui::{
    div, prelude::*, px, AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, Window,
};

use crate::ui::highlight;
use crate::ui::theme::Theme;

/// How far the badge hangs past the picture's bottom-right edge (both axes).
pub const OVERHANG: f32 = 2.0;
/// The ring around the dot, in the surface colour.
pub const RING_WIDTH: f32 = 2.0;
/// The dot is a quarter of the picture, never under 6 nor over 12 (Grok's
/// 12 pt badge on a 48 pt row picture; 9 on the 36 pt footer disc, 6 on the
/// 22 pt header disc).
pub const DOT_MIN: f32 = 6.0;
pub const DOT_MAX: f32 = 12.0;

/// The badge's sizes for a picture of `size` points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub dot: f32,
    pub ring: f32,
    pub overhang: f32,
}

pub fn metrics(size: f32) -> Metrics {
    let dot = (size / 4.0).round().clamp(DOT_MIN, DOT_MAX);
    Metrics {
        dot,
        ring: dot + 2.0 * RING_WIDTH,
        overhang: OVERHANG,
    }
}

/// A dot with its ring.
#[derive(Clone, Debug)]
pub struct Badge {
    /// The dot's colour: the machine state, the typing green at its pulse
    /// alpha, the unread accent.
    pub dot: Hsla,
    /// The ring's colour = the surface the picture rests on (the sidebar, the
    /// selected card, the header pill's glass over the page).
    pub ring: Hsla,
    /// The ring's colour while the hover group the picture sits in is under
    /// the pointer (a row's hover tint), so the ring lifts with the row.
    pub hover: Option<(SharedString, Hsla)>,
    /// A dot size other than [`metrics`]'s (the rail's 8 pt unread dot).
    pub dot_size: Option<f32>,
}

impl Badge {
    pub fn new(dot: Hsla, ring: Hsla) -> Self {
        Self {
            dot,
            ring,
            hover: None,
            dot_size: None,
        }
    }

    pub fn hover(mut self, group: impl Into<SharedString>, ring: Hsla) -> Self {
        self.hover = Some((group.into(), ring));
        self
    }

    pub fn dot_size(mut self, size: f32) -> Self {
        self.dot_size = Some(size);
        self
    }
}

/// Debug selectors the layout tests read.
pub const RING_SELECTOR: &str = "badge-ring";
pub const DOT_SELECTOR: &str = "badge-dot";

/// `picture` in a `size` square with `badge` hanging off its bottom-right
/// corner, in front of it. `None` draws the bare picture in the same box, so
/// a row's layout does not move when the badge comes and goes.
pub fn badged(size: f32, picture: impl IntoElement, badge: Option<Badge>) -> Div {
    let frame = div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(picture);
    match badge {
        Some(badge) => frame.child(above(dot_element(size, badge))),
        None => frame,
    }
}

/// `picture` in a `size` square with any `overlay` (an agent's mini-picture,
/// a stack of guests) hanging `overhang` past its bottom-right corner, in
/// front of it. The overlay brings its own ring.
pub fn corner(
    size: f32,
    picture: impl IntoElement,
    overlay: impl IntoElement,
    overhang: f32,
) -> Div {
    div()
        .relative()
        .size(px(size))
        .flex_shrink_0()
        .child(picture)
        .child(above(
            div()
                .absolute()
                .right(px(-overhang))
                .bottom(px(-overhang))
                .child(overlay),
        ))
}

/// The ring with the dot centred in it, anchored past the corner.
fn dot_element(size: f32, badge: Badge) -> Div {
    let m = metrics(size);
    let dot = badge.dot_size.unwrap_or(m.dot);
    let ring = dot + 2.0 * RING_WIDTH;
    div()
        .absolute()
        .right(px(-m.overhang))
        .bottom(px(-m.overhang))
        .size(px(ring))
        .rounded_full()
        .bg(badge.ring)
        .when_some(badge.hover, |el, (group, color)| {
            // hover-fill: exception badge-ring
            el.group_hover(group, move |s| s.bg(color))
        })
        .flex()
        .items_center()
        .justify_center()
        .debug_selector(|| RING_SELECTOR.into())
        .child(
            div()
                .size(px(dot))
                .rounded_full()
                .bg(badge.dot)
                .debug_selector(|| DOT_SELECTOR.into()),
        )
}

/// `child`, painted in a layer of its own: above everything the enclosing
/// layer paints under it, whatever kind of primitive that is. The child is
/// laid out as if it stood here itself (an `absolute()` child keeps the
/// frame as its anchor), so the layer's box is the child's own.
pub fn above(child: impl IntoElement) -> Above {
    Above {
        child: child.into_any_element(),
    }
}

pub struct Above {
    child: AnyElement,
}

#[cfg(test)]
thread_local! {
    /// Every box an [`Above`] opened a layer for this frame (the layout tests
    /// read it; `debug_bounds` is GPUI's own and not writable from here).
    static LAYERS: std::cell::RefCell<Vec<Bounds<Pixels>>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl IntoElement for Above {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Above {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(test)]
        LAYERS.with(|l| l.borrow_mut().push(bounds));
        window.paint_layer(bounds, |window| self.child.paint(window, cx));
    }
}

/// The ring colour of each surface a badged picture sits on.
pub mod ring {
    use super::*;

    /// The sidebar and the rail at rest: the sidebar colour (on Glass the
    /// tint the sidebar's wash is made of).
    pub fn sidebar(theme: &Theme) -> Hsla {
        theme.sidebar
    }

    /// A selected sidebar row or rail tile: the highlight token's selected
    /// step (`ui::highlight`) as painted over the sidebar.
    pub fn selected(theme: &Theme) -> Hsla {
        highlight::over(theme.sidebar, highlight::fills(theme).selected)
    }

    /// A sidebar row, tile or the footer row under the pointer: the token's
    /// hover step over the sidebar.
    pub fn hovered(theme: &Theme) -> Hsla {
        highlight::over(theme.sidebar, highlight::fills(theme).hover)
    }

    /// The chat header's pill: its glass fill over the page it floats on.
    pub fn header_pill(theme: &Theme) -> Hsla {
        Theme::composite(theme.header_glass(), theme.bg)
    }
}

#[cfg(test)]
mod ordering_tests {
    //! The renderer rule the component relies on, against the pinned GPUI
    //! revision: inside one layer a quad is batched before a photo whatever
    //! the paint order (the bug), and a nested layer is drawn after the
    //! enclosing layer's photo (the fix).
    use gpui::{
        point, size, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask,
        DevicePixels, PolychromeSprite, PrimitiveBatch, Quad, ScaledPixels, Scene, TileId,
    };

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        }
    }

    fn mask() -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: bounds(0.0, 0.0, 1000.0, 1000.0),
        }
    }

    fn photo(b: Bounds<ScaledPixels>) -> PolychromeSprite {
        PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: false.into(),
            opacity: 1.0,
            bounds: b,
            content_mask: mask(),
            corner_radii: Default::default(),
            fade: Default::default(),
            alpha_mask: Default::default(),
            tile: AtlasTile {
                texture_id: AtlasTextureId {
                    index: 0,
                    kind: AtlasTextureKind::Polychrome,
                },
                tile_id: TileId(1),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
                    size: size(DevicePixels(72), DevicePixels(72)),
                },
            },
        }
    }

    fn quad(b: Bounds<ScaledPixels>) -> Quad {
        Quad {
            order: 0,
            border_style: Default::default(),
            bounds: b,
            content_mask: mask(),
            background: Default::default(),
            border_color: Default::default(),
            corner_radii: Default::default(),
            border_widths: Default::default(),
            fade: Default::default(),
        }
    }

    /// The kinds in the order the GPU draws them.
    fn kinds(scene: &mut Scene) -> Vec<&'static str> {
        scene.finish();
        scene
            .batches()
            .map(|b| match b {
                PrimitiveBatch::Quads(_) => "quads",
                PrimitiveBatch::PolychromeSprites { .. } => "photo",
                _ => "other",
            })
            .collect()
    }

    const PANEL: Bounds<ScaledPixels> = Bounds {
        origin: point(ScaledPixels(0.0), ScaledPixels(0.0)),
        size: size(ScaledPixels(300.0), ScaledPixels(800.0)),
    };

    #[test]
    fn inside_one_layer_a_later_quad_still_draws_under_a_photo() {
        let mut scene = Scene::default();
        scene.push_layer(PANEL);
        scene.insert_primitive(photo(bounds(13.0, 700.0, 36.0, 36.0)));
        scene.insert_primitive(quad(bounds(38.0, 725.0, 13.0, 13.0)));
        scene.insert_primitive(quad(bounds(40.0, 727.0, 9.0, 9.0)));
        scene.pop_layer();
        assert_eq!(
            kinds(&mut scene),
            ["quads", "photo"],
            "the frosted sidebar: ring and dot under the photo"
        );
    }

    #[test]
    fn a_nested_layer_draws_after_the_enclosing_layers_photo() {
        let mut scene = Scene::default();
        scene.push_layer(PANEL);
        scene.insert_primitive(photo(bounds(13.0, 700.0, 36.0, 36.0)));
        scene.push_layer(bounds(38.0, 725.0, 13.0, 13.0));
        scene.insert_primitive(quad(bounds(38.0, 725.0, 13.0, 13.0)));
        scene.insert_primitive(quad(bounds(40.0, 727.0, 9.0, 9.0)));
        scene.pop_layer();
        scene.pop_layer();
        assert_eq!(
            kinds(&mut scene),
            ["photo", "quads"],
            "`above`: the badge's layer is drawn after the photo"
        );
    }

    #[test]
    fn outside_any_layer_sibling_order_already_holds() {
        let mut scene = Scene::default();
        scene.insert_primitive(photo(bounds(13.0, 700.0, 36.0, 36.0)));
        scene.insert_primitive(quad(bounds(38.0, 725.0, 13.0, 13.0)));
        scene.insert_primitive(quad(bounds(40.0, 727.0, 9.0, 9.0)));
        assert_eq!(kinds(&mut scene), ["photo", "quads"], "Matte: no layer");
    }

    #[test]
    fn a_nested_layer_keeps_later_siblings_of_the_panel_under_a_popover() {
        // A menu painted after the panel, over the badge, still wins: its
        // order comes from the bounds tree, above the badge's layer.
        let mut scene = Scene::default();
        scene.push_layer(PANEL);
        scene.insert_primitive(photo(bounds(13.0, 700.0, 36.0, 36.0)));
        scene.push_layer(bounds(38.0, 725.0, 13.0, 13.0));
        scene.insert_primitive(quad(bounds(38.0, 725.0, 13.0, 13.0)));
        scene.pop_layer();
        scene.pop_layer();
        scene.insert_primitive(photo(bounds(0.0, 600.0, 200.0, 200.0)));
        assert_eq!(kinds(&mut scene), ["photo", "quads", "photo"]);
    }
}

#[cfg(test)]
mod layout_tests {
    //! The badge's box against its picture, on every surface: it hangs 2 pt
    //! past the bottom-right edge, the ring is 2 pt, the dot is sized to the
    //! picture, and the badge is painted through [`above`].
    use gpui::{Bounds, Pixels, TestAppContext};

    use super::*;
    use crate::ui::theme::{Appearance, Surface};

    struct Fixture {
        size: f32,
        dot_size: Option<f32>,
    }

    impl gpui::Render for Fixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let picture = div()
                .size(px(self.size))
                .rounded_full()
                .debug_selector(|| "picture".into());
            let mut badge = Badge::new(gpui::green(), gpui::black());
            if let Some(d) = self.dot_size {
                badge = badge.dot_size(d);
            }
            div()
                .p(px(20.0))
                .child(badged(self.size, picture, Some(badge)).debug_selector(|| "frame".into()))
        }
    }

    fn f(b: Pixels) -> f32 {
        b.into()
    }

    fn mount(cx: &mut TestAppContext, fixture: Fixture) -> [Bounds<Pixels>; 4] {
        let (_view, vcx) = cx.add_window_view(|_, _| fixture);
        vcx.run_until_parked();
        [
            vcx.debug_bounds("frame").expect("frame drawn"),
            vcx.debug_bounds("picture").expect("picture drawn"),
            vcx.debug_bounds(RING_SELECTOR).expect("ring drawn"),
            vcx.debug_bounds(DOT_SELECTOR).expect("dot drawn"),
        ]
    }

    #[gpui::test]
    fn the_badge_hangs_past_the_pictures_corner_at_every_size(cx: &mut TestAppContext) {
        for size in [22.0, 24.0, 36.0, 48.0, 116.0] {
            let [frame, picture, ring, dot] = mount(
                cx,
                Fixture {
                    size,
                    dot_size: None,
                },
            );
            let m = metrics(size);
            assert_eq!(f(frame.size.width), size, "the frame is the picture's box");
            assert_eq!(picture.origin, frame.origin, "the picture fills the frame");
            assert_eq!(
                f(ring.right()),
                f(picture.right()) + OVERHANG,
                "{size}: right"
            );
            assert_eq!(
                f(ring.bottom()),
                f(picture.bottom()) + OVERHANG,
                "{size}: bottom"
            );
            assert_eq!(f(ring.size.width), m.ring, "{size}: ring box");
            assert_eq!(f(dot.size.width), m.dot, "{size}: dot");
            assert_eq!(
                f(dot.origin.x) - f(ring.origin.x),
                RING_WIDTH,
                "{size}: the ring is {RING_WIDTH} pt"
            );
            assert_eq!(f(dot.origin.y) - f(ring.origin.y), RING_WIDTH);
            // The dot's centre lies past the picture's circle: the badge is
            // read against the surface, not swallowed by the picture.
            let cx_ = f(dot.center().x) - f(picture.center().x);
            let cy_ = f(dot.center().y) - f(picture.center().y);
            assert!(
                (cx_ * cx_ + cy_ * cy_).sqrt() > size / 2.0,
                "{size}: dot centre {cx_},{cy_} vs radius {}",
                size / 2.0
            );
        }
    }

    #[gpui::test]
    fn the_badge_is_painted_in_its_own_layer(cx: &mut TestAppContext) {
        LAYERS.with(|l| l.borrow_mut().clear());
        let (_view, vcx) = cx.add_window_view(|_, _| Fixture {
            size: 36.0,
            dot_size: None,
        });
        vcx.run_until_parked();
        let ring = vcx.debug_bounds(RING_SELECTOR).expect("ring drawn");
        let layers = LAYERS.with(|l| l.borrow().clone());
        assert!(
            layers.contains(&ring),
            "the badge goes through `above`, whose layer is the ring's box: {layers:?} vs {ring:?}"
        );
        assert!(
            layers.iter().all(|b| !b.is_empty()),
            "an empty layer box opens no layer: {layers:?}"
        );
    }

    #[gpui::test]
    fn an_explicit_dot_size_keeps_the_two_point_ring(cx: &mut TestAppContext) {
        let [_, picture, ring, dot] = mount(
            cx,
            Fixture {
                size: 36.0,
                dot_size: Some(8.0),
            },
        );
        assert_eq!(f(dot.size.width), 8.0);
        assert_eq!(f(ring.size.width), 12.0);
        assert_eq!(f(ring.right()), f(picture.right()) + OVERHANG);
    }

    #[test]
    fn the_dot_is_a_quarter_of_the_picture_within_bounds() {
        assert_eq!(metrics(36.0).dot, 9.0, "the footer disc");
        assert_eq!(metrics(36.0).ring, 13.0);
        assert_eq!(metrics(22.0).dot, 6.0, "the header disc");
        assert_eq!(metrics(22.0).ring, 10.0);
        assert_eq!(metrics(48.0).dot, 12.0, "Grok's row picture");
        assert_eq!(metrics(116.0).dot, DOT_MAX, "a hero never grows past 12");
        assert_eq!(metrics(16.0).dot, DOT_MIN);
    }

    /// The ring is the surface's colour on every surface, both appearances
    /// and both surfaces: the sidebar at rest, a selected row, a hovered row,
    /// and the header pill over the page.
    #[test]
    fn the_ring_is_the_surface_colour_per_surface() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            for surface in [Surface::Matte, Surface::Glass] {
                let theme = Theme::with(appearance, surface);
                assert_eq!(ring::sidebar(&theme), theme.sidebar);
                assert_eq!(
                    ring::sidebar(&theme),
                    theme.wash_tint(crate::ui::theme::GlassRegion::Sidebar)
                );
                let fills = highlight::fills(&theme);
                assert_eq!(
                    ring::selected(&theme),
                    highlight::over(theme.sidebar, fills.selected),
                    "the selected step of the one highlight token, over the sidebar"
                );
                let hovered = ring::hovered(&theme);
                assert_eq!(
                    hovered.a, 1.0,
                    "opaque: the ring must hide the picture under it"
                );
                assert_eq!(hovered, highlight::over(theme.sidebar, fills.hover));
                assert_ne!(
                    hovered,
                    ring::selected(&theme),
                    "hover and selected are one step apart"
                );
                let pill = ring::header_pill(&theme);
                assert_eq!(pill.a, 1.0);
                assert_eq!(pill, Theme::composite(theme.header_glass(), theme.bg));
                assert_ne!(pill, theme.bg, "the pill is not the bare page");
            }
        }
    }
}
