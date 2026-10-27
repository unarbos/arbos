//! In-window frosted glass and edge fades on the zui fork primitives
//! (zeron-study §3.2, §3.4). `frosted()` snapshots what is already painted
//! under its bounds, blurs it on the GPU, and paints the child on top — all
//! inside one `paint_layer` so a hover repaint elsewhere cannot reorder the
//! card's quads below the blur. `edge_faded()` fades primitives per pixel near
//! the top/bottom of a scrolling region; over glass there is no background
//! colour to paint a gradient "to", so the fade must be per pixel.

use gpui::{
    px, AnyElement, App, Bounds, Corners, EdgeFade, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Window,
};

use crate::ui::theme::Theme;

pub const MENU_BLUR: f32 = 16.0;
pub const PANEL_BLUR: f32 = 24.0;

pub struct Frosted {
    corner_radius: f32,
    blur_radius: f32,
    /// Blur on Matte too (the chat header's controls, [feedback
    /// mac-header-pill-glass-matte-2026-10-26]); the rest of the glass restyle
    /// follows the surface.
    on_matte: bool,
    child: AnyElement,
}

/// Blur the backdrop under `child` inside a rounded rect, when the theme's
/// surface is Glass. On Matte the child paints as is.
pub fn frosted(corner_radius: f32, blur_radius: f32, child: impl IntoElement) -> Frosted {
    Frosted {
        corner_radius,
        blur_radius,
        on_matte: false,
        child: child.into_any_element(),
    }
}

/// Blur the backdrop under `child` on both surfaces. Matte means no window
/// vibrancy, not a flat header: the chat header's pill and disc are frosted
/// glass whatever the surface (Jacob 2026-10-26).
pub fn frosted_always(corner_radius: f32, blur_radius: f32, child: impl IntoElement) -> Frosted {
    Frosted {
        on_matte: true,
        ..frosted(corner_radius, blur_radius, child)
    }
}

impl IntoElement for Frosted {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Frosted {
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
        if self.on_matte || Theme::of(cx).is_frost() {
            let radius = px(self.corner_radius);
            let blur = px(self.blur_radius);
            window.paint_layer(bounds, |window| {
                window.paint_backdrop_blur(bounds, Corners::all(radius), blur);
                self.child.paint(window, cx);
            });
        } else {
            self.child.paint(window, cx);
        }
    }
}

pub struct EdgeFaded {
    band: f32,
    top: bool,
    bottom: bool,
    child: AnyElement,
}

/// Fade the child's primitives near its top/bottom edges over `band` px.
pub fn edge_faded(band: f32, top: bool, bottom: bool, child: impl IntoElement) -> EdgeFaded {
    EdgeFaded {
        band,
        top,
        bottom,
        child: child.into_any_element(),
    }
}

impl IntoElement for EdgeFaded {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EdgeFaded {
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
        let fade = EdgeFade {
            bounds,
            band: px(self.band),
            band_top: None,
            band_bottom: None,
            band_left: None,
            band_right: None,
            top: self.top,
            bottom: self.bottom,
            left: false,
            right: false,
        };
        window.with_edge_fade(Some(fade), |window| self.child.paint(window, cx));
    }
}
