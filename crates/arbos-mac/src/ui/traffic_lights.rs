//! The window's traffic lights while it is in native full screen.
//!
//! macOS hides a full-screen window's title bar — and with it the close /
//! minimise / zoom buttons — until the pointer touches the top edge, when the
//! bar slides down over the content as an opaque band. In Arbos that left the
//! sidebar's top strip without its lights and, on hover, an empty band above
//! it ([feedback fullscreen-traffic-lights-2026-10-24], Jacob's rule: full
//! screen looks like Chrome's — the window just grows, the lights stay at the
//! top left in the strip's row, nothing shifts).
//!
//! AppKit offers no supported way to keep the real buttons on screen in full
//! screen (Chrome overrides private `NSThemeFrame` methods for it), so while
//! the window is full screen the shell paints its own three lights exactly
//! where the real ones sit in windowed mode ([`LightsGeometry`]) and wires
//! them to the same window actions. The strip itself does not move: see
//! `sidebar::top_strip` and the tests below.

use gpui::{div, prelude::*, px, AnyElement, Hsla, MouseButton, Rgba, Window};

use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Appearance, Theme, HEADER_HEIGHT};

/// Where the real buttons sit in windowed mode, in points from the window's
/// top-left corner. GPUI places the close button's 14 pt frame at
/// `traffic_light_position` (14, 14) — see `main.rs` — and the other two after
/// it at AppKit's own pitch (23 pt on macOS 26, measured on the real build;
/// 20 pt before Tahoe). The 12 pt disc is inset 1 pt in its frame, so the
/// discs are centred 21 pt below the top: 1 pt above the 44 pt strip's middle
/// (`docs/mac-design-rules.md`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightsGeometry {
    /// Left edge of each button frame: close, minimise, zoom.
    pub frame_x: [f32; 3],
    /// Top edge of the button frames.
    pub frame_y: f32,
    /// Side of a (square) button frame.
    pub frame: f32,
    /// Diameter of the coloured disc inside the frame.
    pub disc: f32,
}

/// `traffic_light_position` passed to GPUI in `main.rs`.
pub(crate) const TRAFFIC_LIGHT_POSITION: f32 = 14.0;
/// AppKit's distance between two button frames on macOS 26.
const BUTTON_PITCH: f32 = 23.0;
const BUTTON_FRAME: f32 = 14.0;
const DISC: f32 = 12.0;

impl Default for LightsGeometry {
    fn default() -> Self {
        Self {
            frame_x: [
                TRAFFIC_LIGHT_POSITION,
                TRAFFIC_LIGHT_POSITION + BUTTON_PITCH,
                TRAFFIC_LIGHT_POSITION + 2.0 * BUTTON_PITCH,
            ],
            frame_y: TRAFFIC_LIGHT_POSITION,
            frame: BUTTON_FRAME,
            disc: DISC,
        }
    }
}

impl LightsGeometry {
    /// Centre line of the discs, from the window's top edge.
    pub(crate) fn centre_y(&self) -> f32 {
        self.frame_y + self.frame / 2.0
    }

    /// Left edge of disc `i`.
    fn disc_x(&self, i: usize) -> f32 {
        self.frame_x[i] + (self.frame - self.disc) / 2.0
    }

    /// Top edge of every disc.
    fn disc_y(&self) -> f32 {
        self.centre_y() - self.disc / 2.0
    }

    /// Right edge of the last frame: how much of the strip the lights use.
    pub(crate) fn right(&self) -> f32 {
        self.frame_x[2] + self.frame
    }
}

/// Whether the shell paints its own lights: only in native full screen, only
/// on macOS (elsewhere the window manager keeps its own controls).
pub(crate) fn wanted(fullscreen: bool, macos: bool) -> bool {
    fullscreen && macos
}

/// One of the three lights, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light {
    Close,
    Minimise,
    Zoom,
}

impl Light {
    const ALL: [Light; 3] = [Light::Close, Light::Minimise, Light::Zoom];

    /// Fill and ring, measured on macOS 26's buttons (same values in both
    /// appearances).
    fn colours(self) -> (Rgba, Rgba) {
        match self {
            Light::Close => (gpui::rgb(0xff5c5f), gpui::rgb(0xe0393d)),
            Light::Minimise => (gpui::rgb(0xfac800), gpui::rgb(0xdfa700)),
            Light::Zoom => (gpui::rgb(0x34c759), gpui::rgb(0x00a61c)),
        }
    }

    /// In full screen the minimise button is disabled, as AppKit disables it.
    fn enabled(self) -> bool {
        match self {
            Light::Close | Light::Zoom => true,
            Light::Minimise => false,
        }
    }

    fn element_id(self) -> &'static str {
        match self {
            Light::Close => "traffic-light-close",
            Light::Minimise => "traffic-light-minimise",
            Light::Zoom => "traffic-light-zoom",
        }
    }

    /// The glyph shown while the pointer is over the group (what AppKit draws
    /// on hover): × to close, the inward arrows to leave full screen.
    fn glyph(self) -> Option<&'static str> {
        match self {
            Light::Close => Some("xmark"),
            Light::Minimise => None,
            Light::Zoom => Some("fullscreen-exit"),
        }
    }
}

/// The grey of a disabled or inactive-window light.
fn idle_colours(appearance: Appearance) -> (Rgba, Rgba) {
    match appearance {
        Appearance::Dark => (gpui::rgb(0x414141), gpui::rgb(0x363636)),
        Appearance::Light => (gpui::rgb(0xdcdcdc), gpui::rgb(0xd0d0d0)),
    }
}

/// The lights for the full-screen window, or `None` when the real ones are on
/// screen. Painted over the window's top-left corner, inside the band the
/// sidebar strip (or the folded sidebar's chat header) already leaves clear
/// for them (`TRAFFIC_LIGHTS_INSET`); the strip itself is laid out exactly as
/// in windowed mode.
pub(crate) fn overlay(window: &Window, cx: &mut gpui::Context<Shell>) -> Option<AnyElement> {
    if !wanted(window.is_fullscreen(), cfg!(target_os = "macos")) {
        return None;
    }
    let theme = Theme::of(cx).clone();
    let geometry = LightsGeometry::default();
    let active = window.is_window_active();
    let glyph_colour: Hsla = gpui::rgba(0x0000008c).into();
    let mut group = div()
        .id("traffic-lights")
        .group("traffic-lights")
        .absolute()
        .top_0()
        .left_0()
        .w(px(geometry.right()))
        .h(px(HEADER_HEIGHT))
        // The strip under the lights is a window-drag region; a press on a
        // light is a click, never a move.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (i, light) in Light::ALL.into_iter().enumerate() {
        let lit = active && light.enabled();
        let (fill, ring) = if lit {
            light.colours()
        } else {
            idle_colours(theme.appearance)
        };
        let mut disc = div()
            .id(light.element_id())
            .absolute()
            .left(px(geometry.disc_x(i)))
            .top(px(geometry.disc_y()))
            .size(px(geometry.disc))
            .rounded_full()
            .bg(fill)
            .border(px(0.5))
            .border_color(ring)
            .flex()
            .items_center()
            .justify_center();
        if let Some(glyph) = light.glyph().filter(|_| lit) {
            disc = disc.child(
                div()
                    .invisible()
                    .group_hover("traffic-lights", |s| s.visible())
                    .child(icon(glyph, 8.0, glyph_colour)),
            );
        }
        if lit {
            disc =
                disc.cursor_default()
                    .on_click(cx.listener(move |_, _, window, _| match light {
                        Light::Close => close_window(window),
                        Light::Minimise => {}
                        Light::Zoom => window.toggle_fullscreen(),
                    }));
        }
        group = group.child(disc);
    }
    Some(group.into_any_element())
}

/// What the red light does: AppKit's `performClose:` on our `NSWindow`, so
/// the window closes exactly as it does from the real button (GPUI's
/// should-close hook included). Elsewhere, GPUI's own close.
fn close_window(window: &mut Window) {
    #[cfg(target_os = "macos")]
    {
        if mac::perform_close(window) {
            return;
        }
    }
    window.remove_window();
}

#[cfg(target_os = "macos")]
mod mac {
    use gpui::Window;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// `performClose:` on the window behind the GPUI window; `false` when the
    /// `NSWindow` cannot be reached (the caller falls back to GPUI).
    pub(super) fn perform_close(window: &Window) -> bool {
        let Some(_mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return false;
        };
        // SAFETY: `raw-window-handle` documents the pointer as the live
        // window's NSView*, and we are on the main thread.
        let ns_window = unsafe { appkit.ns_view.cast::<NSView>().as_ref().window() };
        match ns_window {
            Some(ns_window) => {
                ns_window.performClose(None);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::shell::TRAFFIC_LIGHTS_INSET;
    use crate::ui::sidebar::{top_strip, STRIP_INSET};

    #[test]
    fn the_lights_sit_on_the_windowed_line_inside_the_strip() {
        let g = LightsGeometry::default();
        // Centred 1 pt above the 44 pt strip's middle, like the real buttons.
        assert_eq!(g.centre_y(), HEADER_HEIGHT / 2.0 - 1.0);
        assert_eq!(g.disc_y(), 15.0);
        assert_eq!(g.disc_x(0), 15.0);
        assert!(g.disc_y() + g.disc <= HEADER_HEIGHT);
        // All three end before the strip's own content starts (its left pad).
        let clear = TRAFFIC_LIGHTS_INSET + STRIP_INSET;
        assert!(g.right() <= clear, "{} > {clear}", g.right());
        // Close, minimise, zoom follow at one pitch, left to right.
        assert_eq!(g.frame_x[1] - g.frame_x[0], g.frame_x[2] - g.frame_x[1]);
    }

    #[test]
    fn full_screen_adds_no_band_above_the_strip() {
        // The strip is laid out exactly as in windowed mode; the lights are
        // painted into the same band, nothing is pushed down.
        assert_eq!(top_strip(true), top_strip(false));
        let g = LightsGeometry::default();
        assert!(g.centre_y() < top_strip(true));
    }

    #[test]
    fn painted_only_in_native_full_screen_on_macos() {
        assert!(wanted(true, true));
        assert!(!wanted(false, true));
        assert!(!wanted(true, false));
        assert!(!wanted(false, false));
    }

    #[test]
    fn minimise_is_the_only_disabled_light() {
        let disabled: Vec<_> = Light::ALL.into_iter().filter(|l| !l.enabled()).collect();
        assert_eq!(disabled, vec![Light::Minimise]);
        assert_eq!(Light::Close.glyph(), Some("xmark"));
        assert_eq!(Light::Zoom.glyph(), Some("fullscreen-exit"));
        assert_eq!(Light::Minimise.glyph(), None);
    }
}
