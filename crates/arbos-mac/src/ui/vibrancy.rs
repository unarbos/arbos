//! The Glass surface's window material ([feedback mac-glass-contrast-2026-10-26]).
//!
//! GPUI's `WindowBackgroundAppearance::Blurred` is an `NSVisualEffectView`
//! with every tint layer stripped: a colourless gaussian of the desktop. The
//! only tone over it was the shell wash at the Opacity slider's value, so at a
//! low slider a bright desktop bled through and white text, discs and
//! hairlines lost their contrast (dark), and a dark desktop did the same to a
//! light theme. Jacob keeps the slider's floor at 0 % (#1425), so the
//! contrast has to come from the material: on macOS the window carries a real
//! system material (`GlassMaterial`, default `Sidebar` — Finder's sidebar,
//! legible over anything), installed once under GPUI's content view; its
//! `appearance` follows the theme, so an appearance switch re-tones it and
//! never drops it. GPUI's own background goes `Transparent` so the two do not
//! stack. Elsewhere (Linux previews) the plan is computed and nothing is
//! installed.

use gpui::{Window, WindowBackgroundAppearance};

use crate::ui::theme::{Appearance, GlassMaterial, Theme};

/// What the window carries for a theme — pure, read by the tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropPlan {
    /// The system material view under the content, or none (Matte, or
    /// `GlassMaterial::None`).
    pub material: Option<GlassMaterial>,
    /// The appearance the material renders in (the theme's, not macOS's).
    pub appearance: Appearance,
    /// GPUI's own window background.
    pub background: WindowBackgroundAppearance,
}

pub fn plan(theme: &Theme) -> BackdropPlan {
    plan_for(theme, GlassMaterial::current())
}

pub fn plan_for(theme: &Theme, material: GlassMaterial) -> BackdropPlan {
    let wants_material = theme.is_glass() && material != GlassMaterial::None;
    BackdropPlan {
        material: wants_material.then_some(material),
        appearance: theme.appearance,
        background: theme.window_background_for(material),
    }
}

/// Bring the window in line with the theme: install, re-tone or remove the
/// material view, then set GPUI's background. Called after the window opens
/// and after every theme change (`theme::reapply_window_background`).
pub fn apply(window: &mut Window, theme: &Theme) {
    let plan = plan(theme);
    platform::apply(window, &plan);
    window.set_background_appearance(plan.background);
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::BackdropPlan;
    use crate::ui::theme::{Appearance, GlassMaterial};

    thread_local! {
        /// Our material view per window, keyed by the content view's address.
        static VIEWS: RefCell<HashMap<usize, Retained<NSVisualEffectView>>> =
            RefCell::new(HashMap::new());
    }

    fn material(m: GlassMaterial) -> Option<NSVisualEffectMaterial> {
        match m {
            GlassMaterial::Sidebar => Some(NSVisualEffectMaterial::Sidebar),
            GlassMaterial::UnderWindowBackground => {
                Some(NSVisualEffectMaterial::UnderWindowBackground)
            }
            GlassMaterial::HudWindow => Some(NSVisualEffectMaterial::HUDWindow),
            GlassMaterial::Popover => Some(NSVisualEffectMaterial::Popover),
            GlassMaterial::Menu => Some(NSVisualEffectMaterial::Menu),
            GlassMaterial::FullScreenUi => Some(NSVisualEffectMaterial::FullScreenUI),
            GlassMaterial::WindowBackground => Some(NSVisualEffectMaterial::WindowBackground),
            GlassMaterial::None => None,
        }
    }

    fn appearance(a: Appearance) -> Option<Retained<NSAppearance>> {
        // SAFETY: the names are AppKit's exported constants, valid for the process.
        let name = unsafe {
            match a {
                Appearance::Dark => NSAppearanceNameDarkAqua,
                Appearance::Light => NSAppearanceNameAqua,
            }
        };
        NSAppearance::appearanceNamed(name)
    }

    /// The content view of the NSWindow behind a GPUI window.
    fn content_view(window: &gpui::Window) -> Option<Retained<NSView>> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: GPUI's raw window handle is the live window's NSView*
        // (`raw-window-handle` documents it as such).
        let view = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
        view.window()?.contentView()
    }

    pub(super) fn apply(window: &mut gpui::Window, plan: &BackdropPlan) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(content) = content_view(window) else {
            return;
        };
        let key = Retained::as_ptr(&content) as usize;
        match plan.material.and_then(material) {
            None => {
                if let Some(view) = VIEWS.with(|v| v.borrow_mut().remove(&key)) {
                    view.removeFromSuperview();
                }
            }
            Some(wanted) => {
                let view = VIEWS.with(|v| v.borrow().get(&key).cloned());
                let view = match view {
                    Some(view) => view,
                    None => {
                        let frame = content.bounds();
                        let view = NSVisualEffectView::initWithFrame(
                            NSVisualEffectView::alloc(mtm),
                            frame,
                        );
                        view.setAutoresizingMask(
                            NSAutoresizingMaskOptions::ViewWidthSizable
                                | NSAutoresizingMaskOptions::ViewHeightSizable,
                        );
                        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                        // Active always: the material must not go flat when the
                        // window is not key (screenshots, a second window in front).
                        view.setState(NSVisualEffectState::Active);
                        // Under everything GPUI draws (its Metal view is a sibling).
                        content.addSubview_positioned_relativeTo(
                            &view,
                            NSWindowOrderingMode::Below,
                            None,
                        );
                        VIEWS.with(|v| v.borrow_mut().insert(key, view.clone()));
                        view
                    }
                };
                view.setMaterial(wanted);
                view.setAppearance(appearance(plan.appearance).as_deref());
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::BackdropPlan;

    pub(super) fn apply(_window: &mut gpui::Window, _plan: &BackdropPlan) {}
}

/// [feedback mac-glass-contrast-2026-10-26]: the plan the window follows —
/// a system material on Glass that survives an appearance switch (only its
/// appearance changes), nothing on Matte, GPUI's colourless blur only when
/// no material is asked for.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{Surface, Theme};

    #[test]
    fn glass_carries_a_system_material_and_matte_none() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let glass = plan_for(
                &Theme::with(appearance, Surface::Glass),
                GlassMaterial::DEFAULT,
            );
            let matte = plan_for(
                &Theme::with(appearance, Surface::Matte),
                GlassMaterial::DEFAULT,
            );
            if Theme::glass_available() {
                assert_eq!(glass.material, Some(GlassMaterial::Sidebar));
                assert_eq!(glass.background, WindowBackgroundAppearance::Transparent);
            } else {
                assert_eq!(glass.material, None, "no window material off macOS");
            }
            assert_eq!(glass.appearance, appearance);
            assert_eq!(matte.material, None);
            assert_eq!(matte.background, WindowBackgroundAppearance::Opaque);
        }
    }

    /// An appearance switch keeps the material: dark → light → dark changes
    /// only the appearance the material renders in. (Before: GPUI's blur view
    /// was removed and re-added on every change.)
    #[test]
    fn an_appearance_switch_keeps_the_material_and_retones_it() {
        if !Theme::glass_available() {
            return;
        }
        let steps = [Appearance::Dark, Appearance::Light, Appearance::Dark];
        let plans: Vec<_> = steps
            .iter()
            .map(|a| plan_for(&Theme::with(*a, Surface::Glass), GlassMaterial::DEFAULT))
            .collect();
        for (plan, appearance) in plans.iter().zip(steps) {
            assert_eq!(
                plan.material,
                Some(GlassMaterial::DEFAULT),
                "material stays"
            );
            assert_eq!(plan.background, WindowBackgroundAppearance::Transparent);
            assert_eq!(plan.appearance, appearance);
        }
    }

    #[test]
    fn no_material_falls_back_to_gpuis_blur() {
        let plan = plan_for(
            &Theme::with(Appearance::Dark, Surface::Glass),
            GlassMaterial::None,
        );
        assert_eq!(plan.material, None);
        if Theme::glass_available() {
            assert_eq!(plan.background, WindowBackgroundAppearance::Blurred);
        }
    }
}
