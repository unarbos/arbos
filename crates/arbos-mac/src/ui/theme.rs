//! Colours are paint, numbers drive layout (zeron-study §2.4). One global.
//!
//! The palette is the iPhone app's (`apps/ios/.../UI/Theme.swift`), itself
//! measured from Grok Bot: background #141414, assistant bubble #202020, user
//! bubble #5C5C5C, field #181818 with a #242424 rim, system text #95959B.
//! The Mac is that UI scaled up: a chat-list sidebar, one conversation pane.
//! Matte is the default; Glass is a macOS-only toggle (D10, §3.11).

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{hsla, rgb, rgba, App, Global, Hsla, WindowAppearance, WindowBackgroundAppearance};

use crate::sync::credentials::config_dir;

/// Chat-list sidebar (the iPhone home list, one column).
pub const SIDEBAR_WIDTH: f32 = 296.0;
/// Kept for the settings sub-panels, which open just right of the sidebar.
pub const RAIL_WIDTH: f32 = SIDEBAR_WIDTH;
/// The slim conversation header (avatar + name left, icon buttons right); Grok Mac: 44 pt.
pub const HEADER_HEIGHT: f32 = 44.0;
pub const TAB_BAR_HEIGHT: f32 = HEADER_HEIGHT;
/// Composer: one pill with the "+" circle inside left and mic / send inside right.
pub const COMPOSER_WIDTH: f32 = 820.0;
/// Grok Mac (ref 18, calibrated): 48 pt pill, 22 pt side margins, 14 pt below.
pub const COMPOSER_HEIGHT: f32 = 48.0;
pub const COLUMN_MARGIN: f32 = 22.0;
pub const PLUS_BUTTON: f32 = 32.0;
pub const SEND_BUTTON: f32 = 32.0;
/// Icon buttons in the header and sheet corners.
pub const CONTROL: f32 = 32.0;
/// Bubble corners (Grok Mac ≈ 16 pt; the iPhone uses 20).
pub const BUBBLE_RADIUS: f32 = 16.0;
pub const PANEL_RADIUS: f32 = 14.0;
pub const DRAWER_WIDTH: f32 = 320.0;
pub const NARROW_WINDOW: f32 = 1000.0;
pub const GLASS_ALPHA: f32 = 0.80;
pub const FROST_TINT_ALPHA: f32 = 0.50;
/// Type scale (iPhone 17/15/13 pt, scaled for a desktop viewing distance).
pub const TEXT_BODY: f32 = 15.0;
pub const TEXT_SMALL: f32 = 13.0;
pub const TEXT_TINY: f32 = 11.5;
pub const LINE_HEIGHT: f32 = 21.0;
/// Avatar sizes: list row (Grok Mac: 33 pt blobs in a 58 pt row), header, inline sender.
pub const AVATAR_ROW: f32 = 36.0;
pub const AVATAR_HEADER: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Matte,
    Glass,
}

const SURFACE_FILE: &str = "surface";

impl Surface {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "matte" => Some(Self::Matte),
            "glass" => Some(Self::Glass),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Matte => "matte",
            Self::Glass => "glass",
        }
    }

    /// The saved choice, or Matte (D10). Saved as one word in `<config>/surface`
    /// only when picked in Settings, like the appearance.
    pub fn load() -> Self {
        std::fs::read_to_string(config_dir().join(SURFACE_FILE))
            .ok()
            .and_then(|s| Self::parse(&s))
            .unwrap_or(Self::Matte)
    }

    pub fn save(self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(SURFACE_FILE), self.as_str());
    }
}

/// What the window is drawn in right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// What the user chose (D81): System follows macOS and is the default; the
/// order is the Settings order. Saved as one word in `<config>/appearance`
/// only when the user picks in Settings, so a missing file means "never
/// chose" → System, and an explicit `dark` or `light` stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearancePref {
    System,
    Light,
    Dark,
}

const APPEARANCE_FILE: &str = "appearance";

/// macOS's own light/dark, mirrored from the last window-appearance reading so
/// code without a `cx` (notification images) can follow it.
static SYSTEM_DARK: AtomicBool = AtomicBool::new(false);

impl AppearancePref {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// The saved choice, or System.
    pub fn load() -> Self {
        std::fs::read_to_string(config_dir().join(APPEARANCE_FILE))
            .ok()
            .and_then(|s| Self::parse(&s))
            .unwrap_or(Self::System)
    }

    pub fn save(self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(APPEARANCE_FILE), self.as_str());
    }

    /// The appearance to draw: System takes macOS's; anything unknown is Light.
    pub fn resolve(self, system: WindowAppearance) -> Appearance {
        match self {
            Self::Light => Appearance::Light,
            Self::Dark => Appearance::Dark,
            Self::System => match system {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
            },
        }
    }
}

/// Remember macOS's current light/dark (startup and every appearance change).
pub fn note_system_appearance(system: WindowAppearance) {
    SYSTEM_DARK.store(
        matches!(
            system,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
        Ordering::Relaxed,
    );
}

/// Is macOS itself dark right now? Notification banners are drawn by the
/// system, so their pictures follow this rather than the in-app theme (D81).
pub fn system_is_dark() -> bool {
    SYSTEM_DARK.load(Ordering::Relaxed)
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub surface: Surface,
    pub appearance: Appearance,
    /// The user's choice behind `appearance` (System / Light / Dark).
    pub pref: AppearancePref,
    pub bg: Hsla,
    /// The chat-list column (a shade lighter than the conversation, as on Grok Mac).
    pub sidebar: Hsla,
    /// Cards, sheets, rows that lift off the background (iPhone `card`).
    pub surface_1: Hsla,
    /// Hover / selected fills and chips (the assistant-bubble grey).
    pub surface_2: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    /// System lines, timestamps, previews, "is working" (#95959B).
    pub text_muted: Hsla,
    /// Placeholders and footnotes.
    pub text_faint: Hsla,
    /// Grok's blue: unread dots, links, the caret.
    pub accent: Hsla,
    pub accent_soft: Hsla,
    /// User bubble (right).
    pub bubble: Hsla,
    /// Assistant bubble (left).
    pub bubble_assistant: Hsla,
    /// Composer field / glass circle fill and its hairline rim.
    pub field: Hsla,
    pub field_border: Hsla,
    /// Primary filled control (send / voice button): white on dark.
    pub primary: Hsla,
    pub on_primary: Hsla,
    /// Presence green.
    pub online: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub code_bg: Hsla,
    pub font_sans: &'static str,
    pub font_mono: &'static str,
}

impl Global for Theme {}

fn grey(l: f32) -> Hsla {
    hsla(0.0, 0.0, l, 1.0)
}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

fn hex_a(v: u32, a: f32) -> Hsla {
    rgba((v << 8) | ((a * 255.0).round() as u32 & 0xff)).into()
}

impl Theme {
    pub fn dark(surface: Surface) -> Self {
        Self {
            surface,
            appearance: Appearance::Dark,
            pref: AppearancePref::Dark,
            bg: hex(0x141414),
            sidebar: hex(0x1a1a1a),
            surface_1: hex(0x1e1e1e),
            surface_2: hex(0x262626),
            border: hex_a(0xffffff, 0.08),
            border_strong: hex_a(0xffffff, 0.16),
            text: grey(1.0),
            text_muted: hex(0x95959b),
            text_faint: hex(0x6a6a6e),
            accent: hex(0x1084fe),
            accent_soft: hex_a(0x1084fe, 0.18),
            bubble: hex(0x5c5c5c),
            bubble_assistant: hex(0x232323),
            field: hex(0x181818),
            field_border: hex(0x242424),
            primary: grey(1.0),
            on_primary: grey(0.0),
            online: hex(0x34c759),
            warn: hex(0xffb340),
            danger: hex(0xff453a),
            danger_soft: hex_a(0xff453a, 0.14),
            code_bg: hex_a(0x000000, 0.28),
            font_sans: Self::sans_family(),
            font_mono: Self::mono_family(),
        }
    }

    pub fn light(surface: Surface) -> Self {
        Self {
            surface,
            appearance: Appearance::Light,
            pref: AppearancePref::Light,
            // Grok Mac light (ref 16/30): chat #FCFCFC, sidebar #F6F6F6,
            // selected row #EFEFEF, assistant bubble #F2F2F2.
            bg: hex(0xfcfcfc),
            sidebar: hex(0xf6f6f6),
            surface_1: grey(1.0),
            surface_2: hex(0xefefef),
            border: hex_a(0x000000, 0.08),
            border_strong: hex_a(0x000000, 0.16),
            text: hex(0x111111),
            text_muted: hex(0x8a8a8e),
            text_faint: hex(0xa0a0a5),
            accent: hex(0x1084fe),
            accent_soft: hex_a(0x1084fe, 0.14),
            bubble: hex(0x111111),
            bubble_assistant: hex(0xf2f2f2),
            field: grey(1.0),
            field_border: hex(0xe3e3e3),
            primary: hex(0x111111),
            on_primary: grey(1.0),
            online: hex(0x34c759),
            warn: hex(0xe59a00),
            danger: hex(0xd7332a),
            danger_soft: hex_a(0xd7332a, 0.10),
            code_bg: hex_a(0x000000, 0.06),
            font_sans: Self::sans_family(),
            font_mono: Self::mono_family(),
        }
    }

    pub fn of(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }

    pub fn with(appearance: Appearance, surface: Surface) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(surface),
            Appearance::Light => Self::light(surface),
        }
    }

    /// The theme for a choice: System resolves against macOS's current appearance.
    pub fn for_pref(pref: AppearancePref, surface: Surface, cx: &App) -> Self {
        let system = cx.window_appearance();
        note_system_appearance(system);
        let mut theme = Self::with(pref.resolve(system), surface);
        theme.pref = pref;
        theme
    }

    fn sans_family() -> &'static str {
        if cfg!(target_os = "macos") {
            ".SystemUIFont"
        } else {
            "Inter"
        }
    }

    fn mono_family() -> &'static str {
        if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "JetBrains Mono"
        }
    }

    /// Text on a user bubble: white on the dark grey, white on black.
    pub fn bubble_text(&self) -> Hsla {
        grey(1.0)
    }

    /// Glass is only possible where the window can be blurred (D10).
    pub fn glass_available() -> bool {
        cfg!(target_os = "macos")
    }

    /// Window vibrancy: Glass selected and the platform can blur the window.
    pub fn is_glass(&self) -> bool {
        self.surface == Surface::Glass && Self::glass_available()
    }

    /// In-window frosted panels: Glass selected. Works on every backend the
    /// zui fork supports (Metal, wgpu, DirectX), so Linux previews it too.
    pub fn is_frost(&self) -> bool {
        self.surface == Surface::Glass
    }

    /// The shell tint painted on the root (zeron-study §3.1 step 2).
    pub fn shell(&self) -> Hsla {
        let mut c = self.bg;
        c.a = if self.is_glass() { GLASS_ALPHA } else { 1.0 };
        c
    }

    /// Floating cards: translucent tint on frost, flat card on matte (§3.11).
    pub fn card(&self) -> Hsla {
        let mut c = self.surface_1;
        c.a = if self.is_frost() {
            FROST_TINT_ALPHA
        } else {
            1.0
        };
        c
    }

    /// Chrome strips (sidebar, side panels) over frost: thinner than cards.
    pub fn chrome(&self) -> Hsla {
        let mut c = self.sidebar;
        c.a = if self.is_frost() { 0.35 } else { 1.0 };
        c
    }

    /// The glass circle / pill fill of the iPhone header controls: the field
    /// colour at 85 % over the (frosted) background.
    pub fn glass_fill(&self) -> Hsla {
        let mut c = self.field;
        c.a = 0.85;
        c
    }

    pub fn window_background_appearance(&self) -> WindowBackgroundAppearance {
        if self.is_glass() {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        }
    }

    pub fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
        c.a = a;
        c
    }
}

/// GPUI's macOS backend drops the blur view when the appearance changes
/// (zeron-study §3.1 step 4). Re-apply after every theme change, deferred.
pub fn reapply_window_background(cx: &mut App) {
    cx.defer(|cx| {
        let wanted = Theme::of(cx).window_background_appearance();
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| {
                window.set_background_appearance(wanted);
            });
        }
    });
}
