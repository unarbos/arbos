//! Colours are paint, numbers drive layout (zeron-study §2.4). One global.
//!
//! The palette is the iPhone app's (`apps/ios/.../UI/Theme.swift`), itself
//! measured from Grok Bot: background #141414, assistant bubble #202020, user
//! bubble #5C5C5C, field #181818 with a #242424 rim, system text #95959B.
//! The Mac is that UI scaled up: a chat-list sidebar, one conversation pane.
//! Dark + Matte is the default for anyone who never picked (D90); Glass is a
//! macOS-only toggle (D10, §3.11).

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{hsla, rgb, rgba, App, Global, Hsla, WindowAppearance, WindowBackgroundAppearance};

use crate::sync::credentials::config_dir;

/// Chat-list sidebar (the iPhone home list, one column). Default width; the
/// user can drag the right edge between [`SIDEBAR_WIDTH_MIN`] and
/// [`SIDEBAR_WIDTH_MAX`] ([feedback app-01M5MJ9Y91W9ZET71KTCE7PS65]).
pub const SIDEBAR_WIDTH: f32 = 296.0;
/// Narrowest the chat-list column may be dragged (avatar + short name).
pub const SIDEBAR_WIDTH_MIN: f32 = 200.0;
/// Widest the chat-list column may be dragged.
pub const SIDEBAR_WIDTH_MAX: f32 = 480.0;
/// Hit target on the sidebar's right edge for the resize drag.
/// Slightly wider than a hairline so the col-resize cursor is easy to grab
/// ([feedback app-01M5MWRQZ8TM4SS0Q25KSYYVV1]).
pub const SIDEBAR_RESIZE_HANDLE: f32 = 10.0;
/// Kept for the settings sub-panels, which open just right of the sidebar.
pub const RAIL_WIDTH: f32 = SIDEBAR_WIDTH;

const SIDEBAR_WIDTH_FILE: &str = "sidebar_width";
const SIDEBAR_COLLAPSED_FILE: &str = "sidebar_collapsed";

/// Whether the chat list is collapsed to the icon rail (`ui::rail`). Saved
/// as `1` / `0` in `<config>/sidebar_collapsed` on every toggle, so the app
/// reopens as it was left ([feedback mac-sidebar-rail-2026-10-26]). The
/// rig's `ARBOS_SIDEBAR_COLLAPSED=1|0` wins at launch.
pub struct SidebarCollapsed;

impl SidebarCollapsed {
    pub fn load() -> bool {
        if let Ok(v) = std::env::var("ARBOS_SIDEBAR_COLLAPSED") {
            return v.trim() == "1";
        }
        Self::load_from(&config_dir())
    }

    pub fn save(collapsed: bool) {
        Self::save_to(&config_dir(), collapsed);
    }

    /// A missing or unreadable file means expanded (today's default).
    pub fn load_from(dir: &std::path::Path) -> bool {
        std::fs::read_to_string(dir.join(SIDEBAR_COLLAPSED_FILE)).is_ok_and(|s| s.trim() == "1")
    }

    pub fn save_to(dir: &std::path::Path, collapsed: bool) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(
            dir.join(SIDEBAR_COLLAPSED_FILE),
            if collapsed { "1" } else { "0" },
        );
    }
}

#[cfg(test)]
mod sidebar_state_tests {
    use super::SidebarCollapsed;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("arbos-rail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The collapsed state survives a relaunch: saved on every toggle, read
    /// at start, expanded when nothing was ever saved
    /// ([feedback mac-sidebar-rail-2026-10-26]).
    #[test]
    fn the_collapsed_state_is_restored_from_the_config_dir() {
        let dir = scratch("collapsed");
        assert!(
            !SidebarCollapsed::load_from(&dir),
            "nothing saved → expanded"
        );
        SidebarCollapsed::save_to(&dir, true);
        assert!(SidebarCollapsed::load_from(&dir));
        SidebarCollapsed::save_to(&dir, false);
        assert!(!SidebarCollapsed::load_from(&dir));
        std::fs::write(dir.join("sidebar_collapsed"), "garbage").unwrap();
        assert!(
            !SidebarCollapsed::load_from(&dir),
            "a bad file means expanded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The user's chat-list column width. Saved as a float string in
/// `<config>/sidebar_width` when dragged; a missing file keeps
/// [`SIDEBAR_WIDTH`].
pub struct SidebarWidth;

impl SidebarWidth {
    pub fn clamp(v: f32) -> f32 {
        v.round().clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX)
    }

    pub fn load() -> f32 {
        std::fs::read_to_string(config_dir().join(SIDEBAR_WIDTH_FILE))
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map(Self::clamp)
            .unwrap_or(SIDEBAR_WIDTH)
    }

    pub fn save(v: f32) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join(SIDEBAR_WIDTH_FILE),
            format!("{:.0}", Self::clamp(v)),
        );
    }
}
/// Where GPUI puts the close button's frame (`traffic_light_position`, x and
/// y from the window's top-left). macOS 26 draws a 14 pt disc in that frame,
/// so the lights are centred `TRAFFIC_LIGHT_POSITION + 7` pt from the top and
/// follow at a 23 pt pitch. Grok Bot's lights: red disc at x 30–57 / y 32–59
/// px on `ref-grok-round-search-plus.png` (2 px/pt, window top at row 2) —
/// left edge 15, centre 22. We were at 14 / 21 ([feedback
/// topbar-grok-parity-2026-10-24]).
pub const TRAFFIC_LIGHT_POSITION: f32 = 15.0;
/// The button family's centre line, from the window's top edge: Grok Bot's
/// round discs are centred 60 px = **30 pt** under it (same shot: disc rows
/// 24–96 px → top 12, bottom 48), 8 pt below the lights' line. Jacob
/// (2026-10-24): "the top bar on mac is now too high" — ours were centred at
/// 21, in a 44 band ([feedback topbar-grok-parity-2026-10-24]).
pub const CONTROL_LINE: f32 = 30.0;
/// The top band of every pane — the sidebar's strip, the chat header (the
/// centred title pill, also the window-drag catchment) and the right pane's
/// header: the button family centred on [`CONTROL_LINE`], so twice it. Grok's
/// first chat card starts at 64; the sidebar filter row fills the 4 between.
/// History: 44 from Grok's older title row (ref 57: hairline at 44, title at
/// 22); the doubling to 88 (#1226) was too big ([feedback mac-header-height-grok]).
pub const HEADER_HEIGHT: f32 = 2.0 * CONTROL_LINE;
pub const TAB_BAR_HEIGHT: f32 = HEADER_HEIGHT;
/// Composer: one pill with the "+" circle inside left and mic / send inside right.
/// The input's width cap; the pill around it (input + the two 28 pt circles =
/// `transcript::COLUMN_MAX_WIDTH`) is Grok Bot's **960**: 959 pt measured on
/// Jacob's 1720 pt rail shot (849–2766 px at 2 px/pt), 941–948 on two
/// narrower shots — a 960 cap, the side margins below it. The transcript
/// column shares the cap. Was 820 (an 876 pill)
/// ([feedback mac-sidebar-rail-2026-10-26]).
pub const COMPOSER_WIDTH: f32 = 904.0;
/// Grok Mac, measured on the real app (refs 57/58, 2 px/pt): a 44 pt pill with
/// 28 pt circles inset 8, 16 pt side margins and 16 pt below.
pub const COMPOSER_HEIGHT: f32 = 44.0;
pub const COMPOSER_MARGIN: f32 = 16.0;
pub const COMPOSER_BOTTOM: f32 = 16.0;
/// The transcript rows share the composer's side margin and width cap: in
/// Grok the agent bubble's left edge *is* the composer's left edge (refs
/// 57/58: both 16 pt from the pane; the `+` circle sits 8 pt inside the
/// pill). The earlier 21 came from a misread of the same shot
/// ([feedback agent-bubble-inset-2026-10-22]).
pub const COLUMN_MARGIN: f32 = COMPOSER_MARGIN;
pub const PLUS_BUTTON: f32 = 28.0;
pub const SEND_BUTTON: f32 = 28.0;
/// The round-button family: the sidebar strip's search · + · collapse discs,
/// the chat header's sidebar toggle and the header pill's height — one size
/// for all of them, Grok Bot's. Measured on `ref-grok-round-search-plus.png`
/// (2 px/pt): disc 72 px = **36 pt**, glyph stroke 3 px = 1.5 pt, search
/// glyph box 28 px = 14 pt, plus 26 px = 13 pt, discs 16 px = 8 pt apart
/// ([feedback search-palette-2026-10-24]; was 32).
pub const CONTROL: f32 = 36.0;
/// Glyph size inside a [`CONTROL`] disc for the 24-grid icons whose strokes
/// fill ~16–18 of the grid (search, plus): 19 pt draws a 14.3 pt magnifier
/// and a 12.7 pt plus at a 1.6 pt stroke — Grok's 14 / 13 / 1.5 within 1 px.
pub const CONTROL_GLYPH: f32 = 19.0;
/// Glyph size for the wide 24-grid icons (the collapse `sidebar-left` rect
/// fills 20 of 24): 17 pt draws a 14.2 pt box, the magnifier's.
pub const CONTROL_GLYPH_WIDE: f32 = 17.0;
/// Gap between two discs of the family (Grok: 16 px = 8 pt).
pub const CONTROL_GAP: f32 = 8.0;
/// Bubble corners (Grok Mac, measured: 12 pt; the iPhone uses 20).
pub const BUBBLE_RADIUS: f32 = 12.0;
/// Chat bubbles: Grok Mac's body is 13.5 pt on a **22 pt** pitch with 15 pt
/// horizontal and 6 pt vertical padding. The pitch is re-measured on Jacob's
/// 2026-10-24 Grok Bot shots (44 px between the lines of a wrapped agent
/// answer and of a wrapped user message at 2 px/pt; refs 57/58 had read 20)
/// — [feedback bubble-markdown-2026-10-24]. The rest of the app keeps
/// `TEXT_BODY`.
pub const CHAT_BODY: f32 = 13.5;
pub const CHAT_LINE_HEIGHT: f32 = 22.0;
pub const BUBBLE_PAD_X: f32 = 15.0;
pub const BUBBLE_PAD_Y: f32 = 6.0;
pub const PANEL_RADIUS: f32 = 14.0;
pub const DRAWER_WIDTH: f32 = 320.0;
pub const NARROW_WINDOW: f32 = 1000.0;
pub const GLASS_ALPHA: f32 = 0.80;
/// Light + Glass tints the blur with pure white at the same default wash as
/// dark; the chat column adds [`CHAT_WASH_ALPHA`] of its page colour on top so
/// the conversation stays a readable surface while the sidebar and pane show
/// the glass (Finder's model: translucent chrome, opaque content).
pub const GLASS_ALPHA_LIGHT: f32 = 0.80;
/// The thinnest the chat column's / pane's wash gets over glass, however
/// friendly the desktop ([feedback mac-glass-adaptive-2026-10-26]; the fixed
/// 0.85 of [feedback mac-glass-contrast-2026-10-26] is what a white desktop
/// under a dark theme still gets — see `Theme::adaptive_wash`). A quarter of
/// the page colour keeps the conversation reading as a surface.
pub const CHAT_WASH_FLOOR: f32 = 0.25;
/// The sidebar's floor (its fixed value was 0.60 dark / 0.60 light).
pub const CHROME_WASH_FLOOR: f32 = 0.25;
/// Floating cards' floor (menus, sheets, pickers; fixed 0.85 before): a menu
/// is read, so it stays a surface even over a friendly desktop.
pub const CARD_WASH_FLOOR: f32 = 0.55;
/// Glass shell tint: how opaque the window wash is over the blur (Settings →
/// Appearance → Opacity). Default is [`GLASS_ALPHA`] / [`GLASS_ALPHA_LIGHT`].
/// Floor 0 % (#1425; it was 0.30 before, which Jacob hit — the thumb stopped
/// at the track's left end on "30%"). The slider track maps 0…1 straight onto
/// this range. What keeps text readable at 0 % is not the slider: GPUI's own
/// blur view strips the material's tint (a colourless gaussian of the
/// desktop), so since [feedback mac-glass-contrast-2026-10-26] the window
/// carries a real `NSVisualEffectView` system material ([`GlassMaterial`],
/// `ui::vibrancy`) under the wash, and the surfaces that carry text — the
/// chat column, the sidebar, floating cards — have their own tints
/// ([`CHAT_WASH_ALPHA`], [`CHROME_TINT_ALPHA`], [`FROST_TINT_ALPHA`]).
pub const GLASS_OPACITY_MIN: f32 = 0.00;
pub const GLASS_OPACITY_MAX: f32 = 1.00;
const GLASS_OPACITY_FILE: &str = "glass_opacity";
/// Type scale (iPhone 17/15/13 pt, scaled for a desktop viewing distance).
pub const TEXT_BODY: f32 = 15.0;
pub const TEXT_SMALL: f32 = 13.0;
pub const TEXT_TINY: f32 = 11.5;
pub const LINE_HEIGHT: f32 = 21.0;
/// Avatar sizes: list row (Grok Mac: 33 pt blobs in a 58 pt row), header, inline sender.
pub const AVATAR_ROW: f32 = 36.0;
/// Chat header pill picture: the pill is a [`CONTROL`] tall (the strip's
/// button family, Jacob 2026-10-24) and the picture sits 4 pt inside it
/// ([feedback search-palette-2026-10-24]; was 26 in a 34 pt pill, [feedback
/// app-01M5MX8XS2BS7KZCNAS12BXNKD]).
pub const AVATAR_HEADER: f32 = CONTROL - 8.0;

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

    /// The default for anyone who never picked a surface (D10, D90).
    pub const DEFAULT: Self = Self::Matte;

    /// The saved choice, or [`Self::DEFAULT`]. Saved as one word in
    /// `<config>/surface` only when picked in Settings, like the appearance.
    pub fn load() -> Self {
        std::fs::read_to_string(config_dir().join(SURFACE_FILE))
            .ok()
            .and_then(|s| Self::parse(&s))
            .unwrap_or(Self::DEFAULT)
    }

    pub fn save(self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(SURFACE_FILE), self.as_str());
    }
}

/// The macOS window material under the Glass surface ([feedback
/// mac-glass-contrast-2026-10-26], [feedback mac-glass-adaptive-2026-10-26]).
///
/// On macOS 26 the real thing: `NSGlassEffectView`, Liquid Glass, which
/// tones itself to what is behind it ([`Self::Liquid`], the default where
/// the class exists). Before 26, AppKit's semantic `NSVisualEffectView`
/// materials with their tint intact ([`Self::UnderWindowBackground`] is the
/// fallback — the material AppKit documents for the area behind a window's
/// own background). GPUI's own `Blurred` background is an
/// `UnderWindowBackground` view with every tint layer removed; that is
/// [`Self::None`], the colourless blur. Whatever the material, the adaptive
/// washes ([`Theme::adaptive_wash`]) keep the text readable: they are
/// computed as if there were no material at all, so a material can only
/// add.
///
/// Rig: `ARBOS_GLASS_MATERIAL=liquid|sidebar|underwindow|hud|popover|menu|fullscreen|windowbg|none`
/// picks one for a launch (one release, every candidate shot side by side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassMaterial {
    /// macOS 26 Liquid Glass (`NSGlassEffectView`): adaptive by design.
    Liquid,
    /// Finder's and Mail's sidebar: the standard translucent chrome of the
    /// system, legible over anything (#1922's default).
    Sidebar,
    /// AppKit's "behind a window's own background" material (GPUI's choice,
    /// but with the tint kept). The fallback before macOS 26.
    UnderWindowBackground,
    /// The HUD panel material: a heavy dark tone.
    HudWindow,
    Popover,
    Menu,
    FullScreenUi,
    WindowBackground,
    /// No system material: GPUI's colourless blur.
    None,
}

impl GlassMaterial {
    /// The default when nothing is asked for: Liquid Glass where macOS has
    /// it, the under-window material elsewhere (`vibrancy::platform` decides
    /// at install time, since only AppKit knows whether the class exists).
    pub const DEFAULT: Self = Self::Liquid;
    pub const FALLBACK: Self = Self::UnderWindowBackground;

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "liquid" | "liquidglass" | "glasseffect" => Some(Self::Liquid),
            "sidebar" => Some(Self::Sidebar),
            "underwindow" | "underwindowbackground" => Some(Self::UnderWindowBackground),
            "hud" | "hudwindow" => Some(Self::HudWindow),
            "popover" => Some(Self::Popover),
            "menu" => Some(Self::Menu),
            "fullscreen" | "fullscreenui" => Some(Self::FullScreenUi),
            "windowbg" | "windowbackground" => Some(Self::WindowBackground),
            "none" | "gpui" => Some(Self::None),
            _ => Option::None,
        }
    }

    /// The material for this launch: `ARBOS_GLASS_MATERIAL` (rig) or the default.
    pub fn current() -> Self {
        std::env::var("ARBOS_GLASS_MATERIAL")
            .ok()
            .and_then(|v| Self::parse(&v))
            .unwrap_or(Self::DEFAULT)
    }

    /// What to install when the asked-for material is not available on this
    /// macOS (`liquid_available` = the `NSGlassEffectView` class exists).
    pub fn resolve(self, liquid_available: bool) -> Self {
        match self {
            Self::Liquid if !liquid_available => Self::FALLBACK,
            other => other,
        }
    }
}

/// The desktop behind the window, as the app sees it ([feedback
/// mac-glass-adaptive-2026-10-26]): the mean relative luminance (0 black …
/// 1 white) of what lies under the sidebar, the chat column and the right
/// pane — sampled by `ui::backdrop` (`CGWindowListCreateImage` of the screen
/// region below the window, throttled) — so every Glass wash can be as thin
/// as that backdrop allows and as thick as the text needs. `None` = not
/// sampled (first frames, Linux, a failed capture): the washes then assume
/// the worst desktop for the appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backdrop {
    pub left: f32,
    pub center: f32,
    pub right: f32,
}

impl Backdrop {
    /// The same luminance under every band (tests, rigs).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn uniform(l: f32) -> Self {
        Self {
            left: l,
            center: l,
            right: l,
        }
    }
}

/// Where a Glass wash is painted; each region reads its own band of the backdrop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassRegion {
    /// The chat-list sidebar (`chrome()`).
    Sidebar,
    /// The conversation column (`chat_wash()`).
    Chat,
    /// The right info pane (`pane()`).
    Pane,
    /// Floating cards — menus, sheets, pickers (`card()`); they hang over the
    /// chat column, so they read its band.
    Card,
}

/// The user's glass opacity (0.00–1.00). Saved as a float string in
/// `<config>/glass_opacity` when moved in Settings; a missing file keeps the
/// default for the appearance ([`GLASS_ALPHA`] dark, [`GLASS_ALPHA_LIGHT`]
/// light). Only applied while Surface is Glass.
pub struct GlassOpacity;

impl GlassOpacity {
    pub fn clamp(v: f32) -> f32 {
        v.clamp(GLASS_OPACITY_MIN, GLASS_OPACITY_MAX)
    }

    /// The default wash for an appearance when nothing is saved.
    pub fn default_for(appearance: Appearance) -> f32 {
        match appearance {
            Appearance::Dark => GLASS_ALPHA,
            Appearance::Light => GLASS_ALPHA_LIGHT,
        }
    }

    /// The saved choice, or the appearance's default.
    pub fn load(appearance: Appearance) -> f32 {
        std::fs::read_to_string(config_dir().join(GLASS_OPACITY_FILE))
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map(Self::clamp)
            .unwrap_or_else(|| Self::default_for(appearance))
    }

    pub fn save(v: f32) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join(GLASS_OPACITY_FILE),
            format!("{:.2}", Self::clamp(v)),
        );
    }
}

/// What the window is drawn in right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// What the user chose; the order is the Settings order. System follows
/// macOS. Saved as one word in `<config>/appearance` only when the user picks
/// in Settings, so a missing file means "never chose" → [`Self::DEFAULT`]
/// (Dark since D90; System under D81 before it), and an explicit `system`,
/// `dark` or `light` always stays.
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

    /// The default for anyone who never picked a theme (D90).
    pub const DEFAULT: Self = Self::Dark;

    /// The saved choice, or [`Self::DEFAULT`].
    pub fn load() -> Self {
        std::fs::read_to_string(config_dir().join(APPEARANCE_FILE))
            .ok()
            .and_then(|s| Self::parse(&s))
            .unwrap_or(Self::DEFAULT)
    }

    pub fn save(self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(APPEARANCE_FILE), self.as_str());
    }

    /// The appearance to draw: System takes macOS's.
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
    /// Shell tint alpha while Glass is on ([`GlassOpacity`]; default [`GLASS_ALPHA`]).
    pub glass_opacity: f32,
    /// The sampled desktop behind the window ([`Backdrop`]); `None` until
    /// `ui::backdrop` has measured it (and always off macOS).
    pub backdrop: Option<Backdrop>,
    pub bg: Hsla,
    /// The chat-list column (a shade off the conversation, as on Grok Mac).
    pub sidebar: Hsla,
    /// The grouped cards on the right pane (details, members, settings — the
    /// three-pane rule). The pane itself has no tone of its own any more: it
    /// is the chat's surface, as Grok Bot's (dark `#121212` both sides of the
    /// join; [feedback info-pane-chrome-2026-10-24]), so the cards carry the
    /// contrast — dark: the D80 grey (+15 over `bg`, Grok's `#1F1F1F` is +13
    /// over its `#121212`); light: a grey card on the white page.
    pub pane_card: Hsla,
    /// The hairline on the right pane's left edge — Grok Bot's: one device
    /// pixel of `#1F1F1F` on its `#121212` page (+13, ≈ 5 % toward white),
    /// full height ([feedback info-pane-left-hairline-2026-10-25], Jacob
    /// 2026-10-25). Dark: the same step over `bg`; light: the same step toward
    /// black. A colour, not `border`'s alpha, so it reads the same on glass.
    pub pane_edge: Hsla,
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
    /// Grok Bot's update disc: the solid blue circle at the footer's right end
    /// with the white cloud-download glyph (`#326CCB` / `#F5F5F5`, measured
    /// on Jacob's Oct-2026 ref) — [feedback footer-update-button-grok-2026-10-24].
    pub update_disc: Hsla,
    pub on_update_disc: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub code_bg: Hsla,
    /// Code-card token palette — Grok Bot's six code colours, sampled from
    /// Jacob's 2026-10-26 reference ([`code_color`]; `docs/mac-design-rules.md`
    /// § Code cards). `code_guide` is the indent guide, `code_plain` the text.
    pub code_plain: Hsla,
    pub code_keyword: Hsla,
    pub code_function: Hsla,
    pub code_string: Hsla,
    pub code_number: Hsla,
    pub code_comment: Hsla,
    pub code_guide: Hsla,
    pub font_sans: &'static str,
    pub font_mono: &'static str,
    /// The serif face typeset math is painted in (KaTeX's look in Grok Bot).
    pub font_math: &'static str,
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
            glass_opacity: GLASS_ALPHA,
            backdrop: None,
            bg: hex(0x141414),
            sidebar: hex(0x1a1a1a),
            pane_card: hex(0x232323),
            pane_edge: hex(0x212121),
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
            update_disc: hex(0x326ccb),
            on_update_disc: hex(0xf5f5f5),
            warn: hex(0xffb340),
            danger: hex(0xff453a),
            danger_soft: hex_a(0xff453a, 0.14),
            code_bg: hex_a(0x000000, 0.28),
            code_plain: hex(0xd6d6dd),
            code_keyword: hex(0x95d0cd),
            code_function: hex(0xe6b387),
            code_string: hex(0xd898d8),
            code_number: hex(0xf1c972),
            code_comment: hex(0x626262),
            code_guide: hex_a(0xffffff, 0.07),
            font_sans: Self::sans_family(),
            font_mono: Self::mono_family(),
            font_math: Self::math_family(),
        }
    }

    pub fn light(surface: Surface) -> Self {
        Self {
            surface,
            appearance: Appearance::Light,
            pref: AppearancePref::Light,
            glass_opacity: GLASS_ALPHA_LIGHT,
            backdrop: None,
            // Grok Mac light (ref 16/30): chat #FCFCFC, assistant bubble
            // #F2F2F2, white field with a hairline. Jacob (2026-10-22, "strict
            // Grok"): sidebar and right pane both Grok's #F6F6F6, chat #FCFCFC,
            // white cards on the pane; the assistant bubble sits two steps
            // under Grok's so it still reads on the white glass chat.
            // Greys meet WCAG AA on every surface they sit on: muted 5.3:1 on
            // white / 4.7:1 on a bubble, faint 4.6:1 on white, accent 4.8:1 on
            // white (#1084FE was 3.7:1).
            bg: hex(0xfcfcfc),
            sidebar: hex(0xf6f6f6),
            // The pane is the chat's `#FCFCFC` now, so the card takes the
            // grey the pane had (white on white was no card at all).
            pane_card: hex(0xf2f2f2),
            pane_edge: hex(0xefefef),
            surface_1: grey(1.0),
            surface_2: hex(0xe8e8e8),
            border: hex_a(0x000000, 0.10),
            border_strong: hex_a(0x000000, 0.18),
            text: hex(0x111111),
            text_muted: hex(0x6b6b70),
            text_faint: hex(0x757579),
            accent: hex(0x0a6fe0),
            accent_soft: hex_a(0x0a6fe0, 0.14),
            bubble: hex(0x111111),
            bubble_assistant: hex(0xededed),
            field: grey(1.0),
            field_border: hex(0xdedede),
            primary: hex(0x111111),
            on_primary: grey(1.0),
            online: hex(0x34c759),
            // The same blue on the light sidebar (white glyph 5.0:1 on it).
            update_disc: hex(0x326ccb),
            on_update_disc: hex(0xf5f5f5),
            warn: hex(0xe59a00),
            danger: hex(0xd7332a),
            danger_soft: hex_a(0xd7332a, 0.10),
            code_bg: hex_a(0x000000, 0.06),
            code_plain: hex(0x1f1f23),
            code_keyword: hex(0x1f7f7a),
            code_function: hex(0xb8641a),
            code_string: hex(0xa63fa4),
            code_number: hex(0x9a6f00),
            code_comment: hex(0x8c8c8c),
            code_guide: hex_a(0x000000, 0.09),
            font_sans: Self::sans_family(),
            font_mono: Self::mono_family(),
            font_math: Self::math_family(),
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
        theme.glass_opacity = GlassOpacity::load(theme.appearance);
        // Keep the sampled desktop across a theme change: a fresh theme with
        // no sample would paint the worst-case washes until the next sample.
        theme.backdrop = cx.try_global::<Theme>().and_then(|t| t.backdrop);
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

    /// STIX Two ships with macOS (Supplemental fonts) and carries the maths
    /// glyphs — ∑, ≤, ℝ, the small superscripts; Noto Serif is the Linux
    /// stand-in. The renderer lists the fallbacks ([`Self::math_fallbacks`]).
    fn math_family() -> &'static str {
        if cfg!(target_os = "macos") {
            "STIX Two Text"
        } else {
            "Noto Serif"
        }
    }

    /// Faces tried for a glyph the math face lacks, then for the face itself.
    pub fn math_fallbacks() -> Vec<String> {
        if cfg!(target_os = "macos") {
            vec!["STIX Two Math".into(), "Times New Roman".into()]
        } else {
            vec!["DejaVu Serif".into(), "Liberation Serif".into()]
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

    pub fn is_light(&self) -> bool {
        self.appearance == Appearance::Light
    }

    /// The shell tint painted on the root (zeron-study §3.1 step 2). Light
    /// glass tints with pure white, not the off-white page colour: the blur
    /// underneath is already grey, and a grey-on-grey wash is what made Light
    /// + Glass look muddy (Jacob's shot, 2026-10-21).
    pub fn shell(&self) -> Hsla {
        if !self.is_glass() {
            return self.bg;
        }
        let mut c = self.shell_tint();
        c.a = GlassOpacity::clamp(self.glass_opacity);
        c
    }

    /// The colour the shell wash is made of (opaque): white in light, `bg` in dark.
    pub fn shell_tint(&self) -> Hsla {
        if self.is_light() {
            grey(1.0)
        } else {
            self.bg
        }
    }

    /// Floating cards: translucent tint on frost, flat card on matte (§3.11).
    /// On Glass the tint adapts to the desktop ([`Self::adaptive_wash`]).
    pub fn card(&self) -> Hsla {
        if !self.is_frost() {
            return self.surface_1;
        }
        self.adaptive_wash(GlassRegion::Card)
    }

    /// The chat column over glass: a wash of the page colour — white in light,
    /// `bg` in dark — so the conversation is a readable surface and bubbles,
    /// timestamps and the composer stay legible while the sidebar and pane
    /// beside it show the blur (Finder's model: translucent chrome, opaque
    /// content). Light had it since 2026-10-21; dark since [feedback
    /// mac-glass-contrast-2026-10-26]; as thin as the desktop allows since
    /// [feedback mac-glass-adaptive-2026-10-26]. Transparent on Matte — the
    /// column inherits the shell.
    pub fn chat_wash(&self) -> Hsla {
        if !self.is_frost() {
            return gpui::transparent_black();
        }
        self.adaptive_wash(GlassRegion::Chat)
    }

    /// The right pane (three-pane rule) blends with the chat beside it —
    /// Grok Bot's pane is the window, not a lighter panel (Jacob, [feedback
    /// info-pane-chrome-2026-10-24]): the chat column's own wash (nothing on
    /// matte), so both sides of the join are one surface and no hairline is
    /// needed between them — read against the pane's own band of the desktop.
    pub fn pane(&self) -> Hsla {
        if !self.is_frost() {
            return gpui::transparent_black();
        }
        self.adaptive_wash(GlassRegion::Pane)
    }

    /// Chrome strips (sidebar, side panels) over frost: thinner than cards.
    pub fn chrome(&self) -> Hsla {
        if !self.is_frost() {
            return self.sidebar;
        }
        self.adaptive_wash(GlassRegion::Sidebar)
    }

    /// What a region's wash is made of (opaque): the sidebar colour for the
    /// sidebar, the card colour for cards, the page colour otherwise.
    pub fn wash_tint(&self, region: GlassRegion) -> Hsla {
        match region {
            GlassRegion::Sidebar => self.sidebar,
            GlassRegion::Card => self.surface_1,
            GlassRegion::Chat | GlassRegion::Pane => self.shell_tint(),
        }
    }

    /// The thinnest a region's wash may get, however friendly the desktop:
    /// a surface still reads as a surface.
    pub fn wash_floor(region: GlassRegion) -> f32 {
        match region {
            GlassRegion::Sidebar => CHROME_WASH_FLOOR,
            GlassRegion::Chat | GlassRegion::Pane => CHAT_WASH_FLOOR,
            GlassRegion::Card => CARD_WASH_FLOOR,
        }
    }

    /// The sampled luminance under a region, or `None` before the first sample.
    pub fn region_backdrop(&self, region: GlassRegion) -> Option<f32> {
        let b = self.backdrop?;
        Some(match region {
            GlassRegion::Sidebar => b.left,
            GlassRegion::Chat | GlassRegion::Card => b.center,
            GlassRegion::Pane => b.right,
        })
    }

    /// The adaptive wash of a Glass region ([feedback
    /// mac-glass-adaptive-2026-10-26]): the region's tint at the alpha the
    /// text on it needs over *this* desktop — body text ≥ 4.5:1 and muted
    /// text ≥ 3:1 — and never thinner than the region's floor. The desktop
    /// under the region is what the sampler saw, already washed by the shell
    /// at the Opacity slider's value; a desktop that is dark under a dark
    /// theme (or light under a light one) needs nothing beyond the floor, so
    /// the picture shows; a white desktop under white text gets what it
    /// takes. With no sample the worst desktop for the appearance is assumed
    /// (white under dark, black under light), which is #1922's fixed wash.
    /// The window material (`GlassMaterial`) is not credited: it can only
    /// add contrast on top.
    pub fn adaptive_wash(&self, region: GlassRegion) -> Hsla {
        let mut c = self.wash_tint(region);
        c.a = self.wash_alpha(region, self.region_backdrop(region));
        c
    }

    /// The alpha [`Self::adaptive_wash`] paints for a region over a backdrop
    /// of the given luminance (`None` = unknown → the worst case).
    pub fn wash_alpha(&self, region: GlassRegion, backdrop_lum: Option<f32>) -> f32 {
        let worst = if self.is_light() { 0.0 } else { 1.0 };
        let lum = backdrop_lum.unwrap_or(worst).clamp(0.0, 1.0);
        // What the region sits on: the desktop under the shell wash.
        let desktop = srgb_grey_from_luminance(lum);
        let shell = self.shell_tint().to_rgb();
        let s = GlassOpacity::clamp(self.glass_opacity);
        let under = s * shell_grey(shell) + (1.0 - s) * desktop;
        let tint = shell_grey(self.wash_tint(region).to_rgb());
        let body = needed_alpha(tint, under, self.text, 4.5, self.is_light());
        let muted = needed_alpha(tint, under, self.text_muted, 3.0, self.is_light());
        body.max(muted)
            .max(Self::wash_floor(region))
            .clamp(0.0, 1.0)
    }

    /// The glass circle / pill fill of the iPhone header controls: the field
    /// colour at 85 % over the (frosted) background.
    pub fn glass_fill(&self) -> Hsla {
        let mut c = self.field;
        c.a = 0.85;
        c
    }

    /// The chat header's glass ([feedback mac-header-glass-2026-10-25], Jacob
    /// 2026-10-25; the iPhone's `HeaderGlassBackground`): what the name pill
    /// and the header disc are filled with — on Glass and on Matte alike
    /// ([feedback mac-header-pill-glass-matte-2026-10-26]).
    ///
    /// Dark: `#3b3f45` at **30 %** ([feedback
    /// mac-header-pill-see-through-2026-10-26]; was 56 %). Jacob's 14:15 CR
    /// shot of v0.6.275 still read as "opaque dark gray": a 56 % plate over
    /// the flat `bg` is two steps lighter with nothing behind it, and a 16 pt
    /// blur turned whatever scrolled under it to mush. Grok Bot's pill over
    /// its black transcript (Jacob's reference) is only ≈ +12/255 lighter than
    /// the page — the glass is read from its rim and from the text that
    /// passes under it. At 30 % the pill sits ≈ +13 over `#141414`, and the
    /// text behind stays legible through the lighter blur
    /// (`glass::HEADER_BLUR`). White text keeps ≥ 4.5:1 on it: the pill always
    /// lies over the chat column, which is the page colour on Matte and the
    /// 85 % page wash on Glass ([feedback mac-glass-contrast-2026-10-26]), so
    /// its backdrop is never brighter than ≈ `#373737`.
    ///
    /// Light: white at 60 % — over the bright beach backdrop of Jacob's shots
    /// white text measures 6.6:1 on it (`docs/glass-surface-directions.md`,
    /// B), and over the flat Matte `#fcfcfc` the blurred list under it (the
    /// backdrop blur composites ≈ 20 % darker than its source, Metal and wgpu
    /// alike) lands the pill near the `#ededed` of the solid pill it replaced.
    /// Unchanged here: light-theme changes need Jacob's shots first.
    pub fn header_glass(&self) -> Hsla {
        if self.is_light() {
            hex_a(0xffffff, 0.60)
        } else {
            hex_a(0x3b3f45, 0.30)
        }
    }

    /// The 1 pt light rim around a header control on Glass. Dark went 18 % →
    /// 30 % with the thinner fill: like Grok's, the pill's shape is its rim.
    pub fn header_glass_rim(&self) -> Hsla {
        if self.is_light() {
            hex_a(0x000000, 0.10)
        } else {
            hex_a(0xffffff, 0.30)
        }
    }

    /// The specular line along a header control's top edge on Glass.
    pub fn header_glass_highlight(&self) -> Hsla {
        if self.is_light() {
            hex_a(0xffffff, 0.90)
        } else {
            hex_a(0xffffff, 0.45)
        }
    }

    /// What GPUI paints the window with. Glass with a system material
    /// ([`GlassMaterial`]): `Transparent` — the material view under the
    /// content does the blurring and toning (`ui::vibrancy`); GPUI's own
    /// `Blurred` only when no material is wanted. Matte: opaque.
    pub fn window_background_appearance(&self) -> WindowBackgroundAppearance {
        self.window_background_for(GlassMaterial::current())
    }

    pub fn window_background_for(&self, material: GlassMaterial) -> WindowBackgroundAppearance {
        match (self.is_glass(), material) {
            (false, _) => WindowBackgroundAppearance::Opaque,
            (true, GlassMaterial::None) => WindowBackgroundAppearance::Blurred,
            (true, _) => WindowBackgroundAppearance::Transparent,
        }
    }

    /// Floating cards on Glass (menus, sheets, pickers) wear the header's rim
    /// and top highlight ([feedback mac-glass-contrast-2026-10-26]: "rim /
    /// highlight on floating surfaces"); the colours are the header's
    /// (`header_glass_rim`, `header_glass_highlight`), so one glass family.
    pub fn plate_rim(&self) -> Hsla {
        self.header_glass_rim()
    }

    pub fn plate_highlight(&self) -> Hsla {
        self.header_glass_highlight()
    }

    /// The colour a code token paints in (`code_comment` is also italic).
    pub fn code_color(&self, kind: arbos_core::highlight::TokenKind) -> Hsla {
        use arbos_core::highlight::TokenKind;
        match kind {
            TokenKind::Plain => self.code_plain,
            TokenKind::Keyword => self.code_keyword,
            TokenKind::Function => self.code_function,
            TokenKind::String => self.code_string,
            TokenKind::Number => self.code_number,
            TokenKind::Comment => self.code_comment,
        }
    }

    pub fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
        c.a = a;
        c
    }

    /// The same hue, `by` lighter (HSL lightness, clamped): a solid control's
    /// hover and pressed fills — brighter under the hand, brighter still while
    /// the button is down (the chip rule).
    pub fn lighter(mut c: Hsla, by: f32) -> Hsla {
        c.l = (c.l + by).clamp(0.0, 1.0);
        c
    }
}

/// sRGB channel (0–1) → linear light.
fn linear_from_srgb(c: f32) -> f32 {
    if c <= 0.039_28 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light → sRGB channel (0–1).
fn srgb_from_linear(l: f32) -> f32 {
    if l <= 0.003_130_8 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// Relative luminance (WCAG 2.x) of an opaque colour.
pub fn luminance(c: Hsla) -> f32 {
    let rgb = c.to_rgb();
    0.2126 * linear_from_srgb(rgb.r)
        + 0.7152 * linear_from_srgb(rgb.g)
        + 0.0722 * linear_from_srgb(rgb.b)
}

/// WCAG contrast ratio between two opaque colours (1 … 21).
#[cfg(test)]
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The sRGB grey (0–1) with the given relative luminance.
pub fn srgb_grey_from_luminance(lum: f32) -> f32 {
    srgb_from_linear(lum.clamp(0.0, 1.0))
}

/// A colour reduced to one sRGB grey value: the grey of its luminance.
fn shell_grey(c: gpui::Rgba) -> f32 {
    srgb_from_linear(
        0.2126 * linear_from_srgb(c.r)
            + 0.7152 * linear_from_srgb(c.g)
            + 0.0722 * linear_from_srgb(c.b),
    )
}

/// The smallest alpha at which `tint` (an sRGB grey) washed over `under` (an
/// sRGB grey) leaves `text` at least `ratio`:1 against the result. GPUI
/// composites in sRGB, so the wash is a straight mix of the two greys; the
/// contrast is read in linear light. `light` says which way the text wants
/// the surface: a light theme's dark text needs the surface *at least* as
/// bright as the threshold, a dark theme's white text needs it *at most*.
pub fn needed_alpha(tint: f32, under: f32, text: Hsla, ratio: f32, light: bool) -> f32 {
    let lt = luminance(text);
    // A hair over the ratio asked for, so 8-bit rounding never lands under it.
    let ratio = ratio * 1.03;
    // The surface luminance the ratio allows.
    let limit = if light {
        ratio * (lt + 0.05) - 0.05
    } else {
        (lt + 0.05) / ratio - 0.05
    };
    let v = srgb_from_linear(limit.clamp(0.0, 1.0));
    // Mixing toward the tint: v(a) = a·tint + (1−a)·under.
    let ok_already = if light { under >= v } else { under <= v };
    if ok_already {
        return 0.0;
    }
    let reach = if light { tint >= v } else { tint <= v };
    if !reach || (tint - under).abs() < 1e-6 {
        return 1.0;
    }
    ((v - under) / (tint - under)).clamp(0.0, 1.0)
}

/// GPUI's macOS backend drops the blur view when the appearance changes
/// (zeron-study §3.1 step 4). Re-apply after every theme change, deferred:
/// the window material (`ui::vibrancy` — installed once, its appearance
/// follows the theme) and GPUI's own background.
pub fn reapply_window_background(cx: &mut App) {
    cx.defer(|cx| {
        let theme = Theme::of(cx).clone();
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| {
                crate::ui::vibrancy::apply(window, &theme);
            });
        }
    });
}

/// [feedback mac-glass-contrast-2026-10-26] + [feedback
/// mac-glass-adaptive-2026-10-26]: text stays readable on every Glass
/// surface over pure white, pure black and a busy photo desktop (its mean
/// luminance), light and dark, with the Opacity slider at 0, 50 and 100 % —
/// body text ≥ 4.5:1, muted text ≥ 3:1 — counting only what the app paints
/// (no credit for the window material); and the washes are adaptive: over a
/// friendly desktop they sit at their floor, so no heavy fixed tint.
#[cfg(test)]
mod glass_contrast_tests {
    use super::*;

    const SLIDERS: [f32; 3] = [0.0, 0.5, 1.0];

    /// `over` (with its alpha) composited onto the opaque `under`, as GPUI does (sRGB).
    fn composite(over: Hsla, under: Hsla) -> Hsla {
        let (o, u) = (over.to_rgb(), under.to_rgb());
        let a = o.a.clamp(0.0, 1.0);
        gpui::Rgba {
            r: a * o.r + (1.0 - a) * u.r,
            g: a * o.g + (1.0 - a) * u.g,
            b: a * o.b + (1.0 - a) * u.b,
            a: 1.0,
        }
        .into()
    }

    /// The desktops of the matrix: pure white, pure black, and the busy photo
    /// wallpaper of the rig (`wall-busy.jpg`, mean luminance ≈ 0.22 — a mix of
    /// saturated blocks on dark grey; the lake backdrop measures ≈ 0.30).
    fn desktops() -> [(&'static str, f32); 3] {
        [("white", 1.0), ("black", 0.0), ("photo", 0.22)]
    }

    fn themed(appearance: Appearance, slider: f32, lum: Option<f32>) -> Theme {
        let mut t = Theme::with(appearance, Surface::Glass);
        t.glass_opacity = slider;
        t.backdrop = lum.map(Backdrop::uniform);
        t
    }

    /// What a glyph on `region` is drawn over: desktop → shell wash at the
    /// slider → the region's (adaptive) wash.
    fn surface_under(theme: &Theme, region: GlassRegion, lum: f32) -> Hsla {
        let g = srgb_grey_from_luminance(lum);
        let desktop: Hsla = gpui::Rgba {
            r: g,
            g,
            b: g,
            a: 1.0,
        }
        .into();
        let mut shell = theme.shell_tint();
        shell.a = GlassOpacity::clamp(theme.glass_opacity);
        let under = composite(shell, desktop);
        let wash = match region {
            GlassRegion::Sidebar => theme.chrome(),
            GlassRegion::Chat => theme.chat_wash(),
            GlassRegion::Pane => theme.pane(),
            GlassRegion::Card => theme.card(),
        };
        composite(wash, under)
    }

    fn check(region: GlassRegion, theme: &Theme, lum: f32, at: &str) {
        let under = surface_under(theme, region, lum);
        let body = contrast_ratio(theme.text, under);
        let muted = contrast_ratio(theme.text_muted, under);
        assert!(
            body >= 4.5,
            "{region:?} {at}: body text {body:.2}:1 over {:?}",
            under.to_rgb()
        );
        assert!(
            muted >= 3.0,
            "{region:?} {at}: muted text {muted:.2}:1 over {:?}",
            under.to_rgb()
        );
    }

    const REGIONS: [GlassRegion; 4] = [
        GlassRegion::Sidebar,
        GlassRegion::Chat,
        GlassRegion::Pane,
        GlassRegion::Card,
    ];

    /// With the desktop sampled, every region reads over every desktop at
    /// every slider value.
    #[test]
    fn every_region_reads_over_white_black_and_the_photo_at_every_opacity() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            for (wall, lum) in desktops() {
                for slider in SLIDERS {
                    let theme = themed(appearance, slider, Some(lum));
                    for region in REGIONS {
                        let at = format!("{appearance:?} over {wall} at {slider:.1}");
                        check(region, &theme, lum, &at);
                    }
                }
            }
        }
    }

    /// Before the first sample (and off macOS) the washes assume the worst
    /// desktop, so they read over every desktop too.
    #[test]
    fn without_a_sample_the_washes_assume_the_worst_desktop() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            for (wall, lum) in desktops() {
                for slider in SLIDERS {
                    let theme = themed(appearance, slider, None);
                    for region in REGIONS {
                        let at = format!("{appearance:?} unsampled over {wall} at {slider:.1}");
                        check(region, &theme, lum, &at);
                    }
                }
            }
        }
    }

    /// The washes adapt: over the friendly desktop (black under dark, white
    /// under light) every region sits at its floor at 0 %; over the hostile
    /// one it is thick; the photo lands in between — no heavy fixed tint.
    #[test]
    fn the_washes_are_as_thin_as_the_desktop_allows() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let (friendly, hostile) = match appearance {
                Appearance::Dark => (0.0, 1.0),
                Appearance::Light => (1.0, 0.0),
            };
            for region in REGIONS {
                let easy = themed(appearance, 0.0, Some(friendly))
                    .adaptive_wash(region)
                    .a;
                let hard = themed(appearance, 0.0, Some(hostile))
                    .adaptive_wash(region)
                    .a;
                let photo = themed(appearance, 0.0, Some(0.22)).adaptive_wash(region).a;
                assert_eq!(
                    easy,
                    Theme::wash_floor(region),
                    "{appearance:?} {region:?}: the floor over the friendly desktop"
                );
                assert!(
                    hard >= 0.70,
                    "{appearance:?} {region:?}: thick over the hostile desktop ({hard:.2})"
                );
                assert!(
                    easy <= photo && photo <= hard,
                    "{appearance:?} {region:?}: photo between ({easy:.2} ≤ {photo:.2} ≤ {hard:.2})"
                );
            }
            // At 100 % the shell already is the page: nothing beyond the floor.
            for region in REGIONS {
                assert_eq!(
                    themed(appearance, 1.0, Some(hostile))
                        .adaptive_wash(region)
                        .a,
                    Theme::wash_floor(region),
                    "{appearance:?} {region:?}: the floor at 100 %"
                );
            }
        }
    }

    /// Matte is untouched: no wash, no tint — the solid page.
    #[test]
    fn matte_paints_solid_surfaces() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = Theme::with(appearance, Surface::Matte);
            assert_eq!(theme.chat_wash().a, 0.0);
            assert_eq!(theme.pane().a, 0.0);
            assert_eq!(theme.chrome(), theme.sidebar);
            assert_eq!(theme.card(), theme.surface_1);
            assert_eq!(theme.shell(), theme.bg);
            assert_eq!(
                theme.window_background_for(GlassMaterial::DEFAULT),
                WindowBackgroundAppearance::Opaque
            );
        }
    }

    /// The window's background: a system material means GPUI paints nothing
    /// behind the content (the material view does); no material keeps GPUI's
    /// colourless blur. Liquid Glass falls back to the under-window material
    /// where macOS has no `NSGlassEffectView`.
    #[test]
    fn glass_hands_the_backdrop_to_the_system_material() {
        let theme = Theme::with(Appearance::Dark, Surface::Glass);
        if Theme::glass_available() {
            assert_eq!(
                theme.window_background_for(GlassMaterial::Liquid),
                WindowBackgroundAppearance::Transparent
            );
            assert_eq!(
                theme.window_background_for(GlassMaterial::None),
                WindowBackgroundAppearance::Blurred
            );
        } else {
            assert_eq!(
                theme.window_background_for(GlassMaterial::Liquid),
                WindowBackgroundAppearance::Opaque
            );
        }
        assert_eq!(GlassMaterial::parse("liquid"), Some(GlassMaterial::Liquid));
        assert_eq!(
            GlassMaterial::parse("sidebar"),
            Some(GlassMaterial::Sidebar)
        );
        assert_eq!(GlassMaterial::parse("NONE"), Some(GlassMaterial::None));
        assert_eq!(GlassMaterial::parse("marble"), None);
        assert_eq!(GlassMaterial::Liquid.resolve(true), GlassMaterial::Liquid);
        assert_eq!(
            GlassMaterial::Liquid.resolve(false),
            GlassMaterial::UnderWindowBackground
        );
        assert_eq!(
            GlassMaterial::Sidebar.resolve(false),
            GlassMaterial::Sidebar
        );
    }

    #[test]
    fn the_contrast_model_matches_wcag() {
        assert!((contrast_ratio(grey(1.0), grey(0.0)) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(grey(0.5), grey(0.5)) - 1.0).abs() < 0.001);
        let half = composite(Theme::with_alpha(grey(0.0), 0.5), grey(1.0));
        assert!((half.to_rgb().r - 0.5).abs() < 0.01);
        // Round trip of the grey ↔ luminance helpers.
        for l in [0.0_f32, 0.05, 0.22, 0.5, 1.0] {
            let g = srgb_grey_from_luminance(l);
            let back = luminance(
                gpui::Rgba {
                    r: g,
                    g,
                    b: g,
                    a: 1.0,
                }
                .into(),
            );
            assert!((back - l).abs() < 0.002, "{l} → {g} → {back}");
        }
        // needed_alpha: nothing when the surface already passes, everything
        // when the tint itself cannot reach the threshold.
        let white: Hsla = grey(1.0);
        assert_eq!(needed_alpha(0.08, 0.0, white, 4.5, false), 0.0);
        assert_eq!(needed_alpha(0.9, 1.0, white, 4.5, false), 1.0);
    }
}

#[cfg(test)]
mod glass_opacity_tests {
    use super::*;

    /// #1425: the slider's floor is 0 % — the window's blur material carries
    /// the frosted look on its own, so a fully clear tint is still usable.
    #[test]
    fn floor_is_fully_clear_and_ceiling_is_opaque() {
        assert_eq!(GLASS_OPACITY_MIN, 0.0);
        assert_eq!(GLASS_OPACITY_MAX, 1.0);
        assert_eq!(GlassOpacity::clamp(-0.5), GLASS_OPACITY_MIN);
        assert_eq!(GlassOpacity::clamp(0.0), 0.0);
        assert_eq!(GlassOpacity::clamp(1.0), 1.0);
        assert_eq!(GlassOpacity::clamp(1.7), GLASS_OPACITY_MAX);
    }

    /// Values Jacob could not reach before (anything under 30 %) pass through.
    #[test]
    fn values_below_the_old_floor_are_kept() {
        for v in [0.01_f32, 0.10, 0.29] {
            assert_eq!(GlassOpacity::clamp(v), v);
        }
    }

    /// A `glass_opacity` file saved under the old 0.30 floor still reads back
    /// as 0.30, and the appearance defaults sit inside the range.
    #[test]
    fn saved_and_default_values_stay_in_range() {
        assert_eq!(GlassOpacity::clamp(0.30), 0.30);
        for appearance in [Appearance::Dark, Appearance::Light] {
            let d = GlassOpacity::default_for(appearance);
            assert!((GLASS_OPACITY_MIN..=GLASS_OPACITY_MAX).contains(&d));
            assert_eq!(GlassOpacity::clamp(d), d);
        }
    }
}
