//! Rig only: a stand-in desktop under the window where there is no window
//! vibrancy (Linux / Xvfb previews, [feedback right-pane-glass-cards-2026-10-28]).
//!
//! `ARBOS_MOCK_WALLPAPER=<png|jpg>` paints that picture under the whole
//! window on Glass and lets the shell wash go translucent over it, so a
//! headless render shows the surfaces as they compose over a desktop — the
//! picture should already be blurred and toned the way the macOS material
//! would leave it, since nothing here blurs it. `ARBOS_MOCK_BACKDROP_LUM=<0…1>`
//! seeds the sampled backdrop (`Theme::backdrop`) with one luminance, so the
//! adaptive washes take the values they would over that desktop instead of
//! the unsampled worst case. Neither variable does anything on a Mac build's
//! normal path: the real material and sampler run there.

use std::sync::{Arc, OnceLock};

use gpui::{div, img, prelude::*, App, Div, Global, ObjectFit, RenderImage};

use crate::app::decode_image;

const WALLPAPER_VAR: &str = "ARBOS_MOCK_WALLPAPER";
const LUM_VAR: &str = "ARBOS_MOCK_BACKDROP_LUM";

/// Is a stand-in desktop configured for this launch?
pub fn active() -> bool {
    std::env::var_os(WALLPAPER_VAR).is_some()
}

/// The seeded backdrop luminance, if any.
pub fn lum() -> Option<f32> {
    std::env::var(LUM_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|l| l.clamp(0.0, 1.0))
}

struct Wallpaper(Option<Arc<RenderImage>>);

impl Global for Wallpaper {}

fn load() -> Option<Arc<RenderImage>> {
    static LOADED: OnceLock<Option<Arc<RenderImage>>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            let path = std::env::var_os(WALLPAPER_VAR)?;
            let bytes = std::fs::read(path).ok()?;
            decode_image(&bytes).map(Arc::new)
        })
        .clone()
}

/// The picture to paint under the window's content, decoded once.
fn wallpaper(cx: &mut App) -> Option<Arc<RenderImage>> {
    if !cx.has_global::<Wallpaper>() {
        cx.set_global(Wallpaper(load()));
    }
    cx.global::<Wallpaper>().0.clone()
}

/// The layer under everything: the stand-in desktop, filling the window.
/// `None` when no stand-in is configured or the picture did not load.
pub fn layer(cx: &mut App) -> Option<Div> {
    if !active() {
        return None;
    }
    let image = wallpaper(cx)?;
    Some(
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .child(img(image).size_full().object_fit(ObjectFit::Cover)),
    )
}

/// The hook ships inert: without its two variables nothing is painted and
/// the shell wash is what it always was (opaque `bg` off macOS, the glass
/// tint on it); the test suite never sets them.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Appearance, Surface, Theme};

    #[test]
    fn inert_without_the_variables() {
        assert!(std::env::var_os(WALLPAPER_VAR).is_none());
        assert!(std::env::var_os(LUM_VAR).is_none());
        assert!(!active());
        assert_eq!(lum(), None);
        for appearance in [Appearance::Dark, Appearance::Light] {
            let matte = Theme::with(appearance, Surface::Matte);
            assert_eq!(matte.shell(), matte.bg);
            let glass = Theme::with(appearance, Surface::Glass);
            if Theme::glass_available() {
                assert!(glass.shell().a < 1.0, "the glass tint, as before");
            } else {
                assert_eq!(glass.shell(), glass.bg, "no stand-in: opaque off macOS");
            }
            assert_eq!(glass.backdrop, None, "nothing seeded");
        }
    }

    #[gpui::test]
    fn no_layer_without_the_variables(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| assert!(layer(cx).is_none()));
    }
}
