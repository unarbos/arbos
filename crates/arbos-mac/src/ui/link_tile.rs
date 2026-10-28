//! The one link-icon tile ([feedback link-icons-mac-tile-2026-10-27]): every place the Mac
//! shows a site's icon on a tile — a person's Links tab, the Library's Links rows, the search
//! palette's links section, the link-embed cards — draws it from here, so size, corner, fill
//! and the mark's clear space are decided once and read the same in the dark and in the light.
//!
//! Jacob's report (2026-10-27, dark): the Spotify row had a crisp green brand mark, the
//! github.com row a blurry 16 px favicon stretched over a 36 pt tile, black octocat on a dark
//! grey square. Three rules fix that for good:
//!
//! 1. **Known hosts draw a bundled vector mark**, never a fetched picture: GitHub, X, YouTube,
//!    Spotify, Apple Music, SoundCloud, Vimeo, Loom, Bluesky, Reddit, Google Docs / Sheets /
//!    Slides / Maps, Figma, Notion, Linear, Wikipedia and Arbos itself ([`Brand`]). Each mark is
//!    one filled path (`assets/brands/<slug>.svg`) painted through GPUI's `svg()`, so it is sharp
//!    at any scale and takes the colour the theme needs: a brand with a colour of its own keeps it
//!    (Spotify green, YouTube red…); a black-and-white brand (GitHub, X, Notion, Wikipedia,
//!    Arbos) is white on the dark tile and near-black on the light one — a dark mark never sits
//!    on a dark fill. The marks come from the brands' own press pages via the Simple Icons
//!    collection (CC0 for the collection; each mark stays its owner's trademark and is used as
//!    its guidelines allow: unaltered shape, brand colour or mono, clear space around it);
//!    sources per brand are in [`Brand::source`].
//! 2. **Unknown hosts draw the relay's high-resolution icon** (`GET /link-icon?domain=`, the
//!    site's apple-touch-icon or largest declared icon, never a 16 px favicon enlarged) filling
//!    the tile edge to edge, resampled on the CPU to the tile's device pixels
//!    ([`crate::ui::picture_fit`]). A fetched icon that is a transparent dark mark gets a light
//!    tile fill in the dark theme (and a light mark a dark fill in the light theme), by its
//!    [`Tone`], measured once when the PNG is decoded.
//! 3. **No icon yet, or none at all** → a neutral tile with the host's first letter in the
//!    muted text colour — never a coloured disc, never an old picture.
//!
//! Metrics: the tile is a rounded square of the caller's `size` (36 pt on pane rows, 32 in the
//! Library, 14 inline) with corner [`radius`] = 28 % of the side (36 → 10, the pane kit's
//! corner; 14 → 4, the inline icon's quarter), filled with the theme's `surface_2` (the chip
//! grey; light: the light grey chip) unless a tone asks otherwise. A brand mark is
//! [`MARK_RATIO`] of the side (36 → 20 pt), centred, so 22 % of clear space surrounds it on
//! every side.

use std::sync::Arc;

use gpui::{div, prelude::*, px, svg, Div, FontWeight, Hsla, RenderImage, SharedString, Svg};

use crate::ui::picture_fit;
use crate::ui::theme::Theme;

/// The mark's side as a share of the tile's: 22 % clear space on every side.
pub const MARK_RATIO: f32 = 0.56;
/// The tile's corner as a share of its side (36 → 10, the pane kit's corner).
pub const RADIUS_RATIO: f32 = 0.28;
/// The letter on the neutral tile, as a share of the side (36 → 15 pt).
const LETTER_RATIO: f32 = 0.42;

/// The tile's corner radius for a `size` pt tile.
pub fn radius(size: f32) -> f32 {
    (size * RADIUS_RATIO).round().max(2.0)
}

/// A brand whose mark ships with the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Brand {
    GitHub,
    X,
    YouTube,
    Spotify,
    AppleMusic,
    SoundCloud,
    Vimeo,
    Loom,
    Bluesky,
    Reddit,
    GoogleDocs,
    GoogleSheets,
    GoogleSlides,
    GoogleMaps,
    Figma,
    Notion,
    Linear,
    Wikipedia,
    Arbos,
}

/// Every brand, for the asset source and the tests.
pub const BRANDS: [Brand; 19] = [
    Brand::GitHub,
    Brand::X,
    Brand::YouTube,
    Brand::Spotify,
    Brand::AppleMusic,
    Brand::SoundCloud,
    Brand::Vimeo,
    Brand::Loom,
    Brand::Bluesky,
    Brand::Reddit,
    Brand::GoogleDocs,
    Brand::GoogleSheets,
    Brand::GoogleSlides,
    Brand::GoogleMaps,
    Brand::Figma,
    Brand::Notion,
    Brand::Linear,
    Brand::Wikipedia,
    Brand::Arbos,
];

/// How a mark takes colour on the tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// A black-and-white brand: the theme's text colour (white on dark, near-black on light).
    Mono,
    /// The brand's own colour (sRGB hex), the same on both tiles.
    Color(u32),
}

impl Brand {
    /// The brand for a URL — host first, then the path for hosts that serve several products
    /// (`docs.google.com/document` vs `/spreadsheets` vs `/presentation`). `None` for every
    /// other site, which draws the relay's icon or the letter tile.
    pub fn for_url(url: &str) -> Option<Brand> {
        let rest = url
            .trim()
            .strip_prefix("https://")
            .or_else(|| url.trim().strip_prefix("http://"))
            .unwrap_or(url.trim());
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host);
        let path = path.to_ascii_lowercase();
        if host == "docs.google.com" {
            return Some(if path.starts_with("spreadsheets") {
                Brand::GoogleSheets
            } else if path.starts_with("presentation") {
                Brand::GoogleSlides
            } else {
                Brand::GoogleDocs
            });
        }
        if host == "google.com" && path.starts_with("maps") {
            return Some(Brand::GoogleMaps);
        }
        Brand::for_host(host)
    }

    /// The brand for a bare host (no path), `www.` or not. `docs.google.com` alone reads as
    /// Google Docs; [`Brand::for_url`] tells the three Google editors apart.
    pub fn for_host(host: &str) -> Option<Brand> {
        let host = host.trim().to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host);
        let ends = |suffix: &str| host == suffix || host.ends_with(&format!(".{suffix}"));
        Some(match host {
            "github.com" | "gist.github.com" | "raw.githubusercontent.com" => Brand::GitHub,
            "x.com" | "twitter.com" | "t.co" | "mobile.twitter.com" => Brand::X,
            "youtube.com" | "youtu.be" | "m.youtube.com" | "music.youtube.com" => Brand::YouTube,
            "open.spotify.com" | "spotify.com" | "play.spotify.com" => Brand::Spotify,
            "music.apple.com" | "itunes.apple.com" => Brand::AppleMusic,
            "soundcloud.com" | "on.soundcloud.com" | "m.soundcloud.com" => Brand::SoundCloud,
            "vimeo.com" | "player.vimeo.com" => Brand::Vimeo,
            "loom.com" => Brand::Loom,
            "bsky.app" | "bsky.social" => Brand::Bluesky,
            "reddit.com" | "old.reddit.com" | "redd.it" | "np.reddit.com" => Brand::Reddit,
            "docs.google.com" => Brand::GoogleDocs,
            "sheets.google.com" => Brand::GoogleSheets,
            "slides.google.com" => Brand::GoogleSlides,
            "maps.google.com" | "maps.app.goo.gl" => Brand::GoogleMaps,
            "figma.com" => Brand::Figma,
            "notion.so" | "notion.site" | "notion.com" => Brand::Notion,
            "linear.app" => Brand::Linear,
            "wikipedia.org" | "wikimedia.org" => Brand::Wikipedia,
            "arbos.life" => Brand::Arbos,
            _ => {
                if ends("wikipedia.org") {
                    Brand::Wikipedia
                } else if ends("notion.site") {
                    Brand::Notion
                } else if ends("arbos.life") {
                    Brand::Arbos
                } else if ends("loom.com") {
                    Brand::Loom
                } else if ends("figma.com") {
                    Brand::Figma
                } else if ends("linear.app") {
                    Brand::Linear
                } else {
                    return None;
                }
            }
        })
    }

    /// The asset path under the app's asset source (`brands/<slug>.svg`).
    pub fn asset(self) -> &'static str {
        match self {
            Brand::GitHub => "brands/github.svg",
            Brand::X => "brands/x.svg",
            Brand::YouTube => "brands/youtube.svg",
            Brand::Spotify => "brands/spotify.svg",
            Brand::AppleMusic => "brands/apple-music.svg",
            Brand::SoundCloud => "brands/soundcloud.svg",
            Brand::Vimeo => "brands/vimeo.svg",
            Brand::Loom => "brands/loom.svg",
            Brand::Bluesky => "brands/bluesky.svg",
            Brand::Reddit => "brands/reddit.svg",
            Brand::GoogleDocs => "brands/google-docs.svg",
            Brand::GoogleSheets => "brands/google-sheets.svg",
            Brand::GoogleSlides => "brands/google-slides.svg",
            Brand::GoogleMaps => "brands/google-maps.svg",
            Brand::Figma => "brands/figma.svg",
            Brand::Notion => "brands/notion.svg",
            Brand::Linear => "brands/linear.svg",
            Brand::Wikipedia => "brands/wikipedia.svg",
            Brand::Arbos => "brands/arbos.svg",
        }
    }

    /// The SVG behind [`Brand::asset`], for the asset source.
    pub fn svg(self) -> &'static str {
        match self {
            Brand::GitHub => include_str!("../../../../assets/brands/github.svg"),
            Brand::X => include_str!("../../../../assets/brands/x.svg"),
            Brand::YouTube => include_str!("../../../../assets/brands/youtube.svg"),
            Brand::Spotify => include_str!("../../../../assets/brands/spotify.svg"),
            Brand::AppleMusic => include_str!("../../../../assets/brands/apple-music.svg"),
            Brand::SoundCloud => include_str!("../../../../assets/brands/soundcloud.svg"),
            Brand::Vimeo => include_str!("../../../../assets/brands/vimeo.svg"),
            Brand::Loom => include_str!("../../../../assets/brands/loom.svg"),
            Brand::Bluesky => include_str!("../../../../assets/brands/bluesky.svg"),
            Brand::Reddit => include_str!("../../../../assets/brands/reddit.svg"),
            Brand::GoogleDocs => include_str!("../../../../assets/brands/google-docs.svg"),
            Brand::GoogleSheets => include_str!("../../../../assets/brands/google-sheets.svg"),
            Brand::GoogleSlides => include_str!("../../../../assets/brands/google-slides.svg"),
            Brand::GoogleMaps => include_str!("../../../../assets/brands/google-maps.svg"),
            Brand::Figma => include_str!("../../../../assets/brands/figma.svg"),
            Brand::Notion => include_str!("../../../../assets/brands/notion.svg"),
            Brand::Linear => include_str!("../../../../assets/brands/linear.svg"),
            Brand::Wikipedia => include_str!("../../../../assets/brands/wikipedia.svg"),
            Brand::Arbos => include_str!("../../../../assets/brands/arbos.svg"),
        }
    }

    /// The brand's display name (the provider line on an embed card, ARB-468).
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Brand::GitHub => "GitHub",
            Brand::X => "X",
            Brand::YouTube => "YouTube",
            Brand::Spotify => "Spotify",
            Brand::AppleMusic => "Apple Music",
            Brand::SoundCloud => "SoundCloud",
            Brand::Vimeo => "Vimeo",
            Brand::Loom => "Loom",
            Brand::Bluesky => "Bluesky",
            Brand::Reddit => "Reddit",
            Brand::GoogleDocs => "Google Docs",
            Brand::GoogleSheets => "Google Sheets",
            Brand::GoogleSlides => "Google Slides",
            Brand::GoogleMaps => "Google Maps",
            Brand::Figma => "Figma",
            Brand::Notion => "Notion",
            Brand::Linear => "Linear",
            Brand::Wikipedia => "Wikipedia",
            Brand::Arbos => "Arbos",
        }
    }

    /// How the mark takes colour: the brand's own colour where it has one (the hex its press
    /// page publishes), mono for the black-and-white brands.
    pub fn ink(self) -> Ink {
        match self {
            Brand::GitHub | Brand::X | Brand::Notion | Brand::Wikipedia | Brand::Arbos => Ink::Mono,
            Brand::YouTube => Ink::Color(0xFF0000),
            Brand::Spotify => Ink::Color(0x1ED760),
            Brand::AppleMusic => Ink::Color(0xFA243C),
            Brand::SoundCloud => Ink::Color(0xFF5500),
            Brand::Vimeo => Ink::Color(0x1AB7EA),
            Brand::Loom => Ink::Color(0x625DF5),
            Brand::Bluesky => Ink::Color(0x1185FE),
            Brand::Reddit => Ink::Color(0xFF4500),
            Brand::GoogleDocs => Ink::Color(0x4285F4),
            Brand::GoogleSheets => Ink::Color(0x34A853),
            Brand::GoogleSlides => Ink::Color(0xFBBC04),
            Brand::GoogleMaps => Ink::Color(0x4285F4),
            Brand::Figma => Ink::Color(0xF24E1E),
            Brand::Linear => Ink::Color(0x5E6AD2),
        }
    }

    /// Where the mark and its usage terms come from (noted on the issue / PR; the tests check
    /// every brand names one).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn source(self) -> &'static str {
        match self {
            Brand::GitHub => "https://github.com/logos",
            Brand::X => "https://about.x.com/en/who-we-are/brand-toolkit",
            Brand::YouTube => "https://www.youtube.com/howyoutubeworks/resources/brand-resources/",
            Brand::Spotify => "https://developer.spotify.com/documentation/design",
            Brand::AppleMusic => {
                "https://www.apple.com/itunes/marketing-on-music/identity-guidelines.html"
            }
            Brand::SoundCloud => "https://press.soundcloud.com",
            Brand::Vimeo => "https://press.vimeo.com/brand-guidelines",
            Brand::Loom => "https://www.loom.com/press",
            Brand::Bluesky => "https://bsky.social/about/blog/press-faq",
            Brand::Reddit => "https://www.redditinc.com/brand",
            Brand::GoogleDocs | Brand::GoogleSheets | Brand::GoogleSlides | Brand::GoogleMaps => {
                "https://about.google/brand-resource-center/"
            }
            Brand::Figma => "https://www.figma.com/using-the-figma-brand/",
            Brand::Notion => "https://www.notion.so/press",
            Brand::Linear => "https://linear.app/brand",
            Brand::Wikipedia => "https://commons.wikimedia.org/wiki/File:Wikipedia-logo-v2.svg",
            Brand::Arbos => "site/assets/logo/arbos-mark.svg (first party)",
        }
    }

    /// The colour the mark is painted in on this theme's tile.
    pub fn color(self, theme: &Theme) -> Hsla {
        match self.ink() {
            Ink::Mono => theme.text,
            Ink::Color(hex) => gpui::rgb(hex).into(),
        }
    }
}

/// The SVG bytes behind a `brands/<slug>.svg` asset path, for the asset source.
pub fn brand_asset_bytes(path: &str) -> Option<&'static [u8]> {
    BRANDS
        .iter()
        .find(|b| b.asset() == path)
        .map(|b| b.svg().as_bytes())
}

/// What a fetched icon looks like against a fill, measured once from its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    /// Opaque, or a mid-tone mark: fine on the theme's own tile fill.
    #[default]
    Neutral,
    /// A transparent picture whose paint is dark: needs a light fill.
    DarkMark,
    /// A transparent picture whose paint is light: needs a dark fill.
    LightMark,
}

/// The tone of an RGBA picture: transparent pixels over [`TRANSPARENT_SHARE`] of the area make
/// it a *mark*; the mean luminance of its opaque paint then says dark or light.
pub fn tone_of(rgba: &image::RgbaImage) -> Tone {
    let total = (rgba.width() * rgba.height()).max(1) as f32;
    let mut clear = 0u32;
    let mut lum = 0f32;
    let mut painted = 0u32;
    for p in rgba.pixels() {
        if p[3] < 128 {
            clear += 1;
        } else {
            painted += 1;
            lum += 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        }
    }
    if (clear as f32) / total < TRANSPARENT_SHARE || painted == 0 {
        return Tone::Neutral;
    }
    let mean = lum / painted as f32 / 255.0;
    if mean < DARK_MARK_MAX {
        Tone::DarkMark
    } else if mean > LIGHT_MARK_MIN {
        Tone::LightMark
    } else {
        Tone::Neutral
    }
}

/// A picture is a mark (not a full-bleed icon) when at least this share of it is transparent.
pub const TRANSPARENT_SHARE: f32 = 0.08;
/// Mean luminance (0–1) under which a mark's paint counts as dark.
pub const DARK_MARK_MAX: f32 = 0.35;
/// Mean luminance over which a mark's paint counts as light.
pub const LIGHT_MARK_MIN: f32 = 0.72;

/// What the tile shows.
pub enum Icon<'a> {
    /// A bundled vector mark.
    Brand(Brand),
    /// The relay's icon for the host, with its tone.
    Picture(&'a Arc<RenderImage>, Tone),
    /// Nothing yet (loading) or nothing at all: the host's first letter (the host, as shown).
    Initial(String),
}

/// The tile's fill for an icon on this theme: the chip grey, flipped when a transparent mark
/// would vanish on it.
pub fn fill(icon: &Icon<'_>, theme: &Theme) -> Hsla {
    match icon {
        Icon::Picture(_, Tone::DarkMark) if !theme.is_light() => light_fill(),
        Icon::Picture(_, Tone::LightMark) if theme.is_light() => dark_fill(),
        _ => theme.surface_2,
    }
}

/// The light fill under a dark transparent mark in the dark theme (Grok's near-white chip).
fn light_fill() -> Hsla {
    gpui::rgb(0xECECEE).into()
}

/// The dark fill under a light transparent mark in the light theme.
fn dark_fill() -> Hsla {
    gpui::rgb(0x2C2C2E).into()
}

/// The icon for a link: its brand when the host is known, else the relay's picture when it has
/// arrived, else the host's initial.
pub fn icon_for<'a>(
    url: &str,
    host: &str,
    picture: Option<(&'a Arc<RenderImage>, Tone)>,
) -> Icon<'a> {
    if let Some(brand) = Brand::for_url(url) {
        return Icon::Brand(brand);
    }
    match picture {
        Some((image, tone)) => Icon::Picture(image, tone),
        None => Icon::Initial(host.to_owned()),
    }
}

/// The tile: a `size` pt rounded square with the icon on it.
pub fn tile(icon: Icon<'_>, size: f32, theme: &Theme) -> Div {
    let r = radius(size);
    let base = div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(r))
        .overflow_hidden()
        .bg(fill(&icon, theme))
        .flex()
        .items_center()
        .justify_center();
    match icon {
        Icon::Brand(brand) => base.child(
            div()
                .size(px(mark_side(size)))
                .flex_shrink_0()
                .debug_selector(|| MARK_SELECTOR.into())
                .child(mark(brand, size, theme)),
        ),
        Icon::Picture(image, _) => base.child(
            gpui::img(picture_fit::fit(image, size))
                .size(px(size))
                .rounded(px(r))
                .object_fit(gpui::ObjectFit::Cover),
        ),
        Icon::Initial(host) => base
            .text_size(px((size * LETTER_RATIO).round()))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text_muted)
            .child(SharedString::from(initial(&host))),
    }
}

/// The debug selector on the box a brand mark is centred in (headless tests measure it).
pub const MARK_SELECTOR: &str = "link-tile-mark";

/// The mark's side on a `size` pt tile.
pub fn mark_side(size: f32) -> f32 {
    (size * MARK_RATIO).round()
}

/// The brand's mark alone, sized for a `size` pt tile, in the theme's ink for it (the embed
/// cards' provider glyph draws this without a tile).
pub fn mark(brand: Brand, size: f32, theme: &Theme) -> Svg {
    svg()
        .path(SharedString::from(brand.asset()))
        .size(px(mark_side(size)))
        .flex_shrink_0()
        .text_color(brand.color(theme))
}

/// The letter on a neutral tile: the host's first letter, upper-cased; `#` for nothing.
pub fn initial(host: &str) -> String {
    host.trim_start_matches("www.")
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "#".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_map_to_their_brand_with_and_without_www_and_scheme() {
        assert_eq!(
            Brand::for_url("https://github.com/unarbos/arbos"),
            Some(Brand::GitHub)
        );
        assert_eq!(
            Brand::for_url("https://www.github.com/x"),
            Some(Brand::GitHub)
        );
        assert_eq!(
            Brand::for_url("http://gist.github.com/a/b"),
            Some(Brand::GitHub)
        );
        assert_eq!(
            Brand::for_url("https://x.com/arbos/status/1"),
            Some(Brand::X)
        );
        assert_eq!(Brand::for_url("https://twitter.com/arbos"), Some(Brand::X));
        assert_eq!(
            Brand::for_url("https://youtu.be/dQw4w9WgXcQ"),
            Some(Brand::YouTube)
        );
        assert_eq!(
            Brand::for_url("https://music.youtube.com/watch?v=1"),
            Some(Brand::YouTube)
        );
        assert_eq!(
            Brand::for_url("https://open.spotify.com/track/1?si=x"),
            Some(Brand::Spotify)
        );
        assert_eq!(
            Brand::for_url("https://music.apple.com/us/album/1"),
            Some(Brand::AppleMusic)
        );
        assert_eq!(
            Brand::for_url("https://soundcloud.com/a/b"),
            Some(Brand::SoundCloud)
        );
        assert_eq!(Brand::for_url("https://vimeo.com/1"), Some(Brand::Vimeo));
        assert_eq!(
            Brand::for_url("https://www.loom.com/share/1"),
            Some(Brand::Loom)
        );
        assert_eq!(
            Brand::for_url("https://bsky.app/profile/a"),
            Some(Brand::Bluesky)
        );
        assert_eq!(
            Brand::for_url("https://old.reddit.com/r/rust"),
            Some(Brand::Reddit)
        );
        assert_eq!(
            Brand::for_url("https://www.figma.com/file/1"),
            Some(Brand::Figma)
        );
        assert_eq!(
            Brand::for_url("https://arbos.notion.site/page"),
            Some(Brand::Notion)
        );
        assert_eq!(
            Brand::for_url("https://linear.app/arbos/issue/ARB-1"),
            Some(Brand::Linear)
        );
        assert_eq!(
            Brand::for_url("https://en.wikipedia.org/wiki/Tree"),
            Some(Brand::Wikipedia)
        );
        assert_eq!(
            Brand::for_url("https://arbos.life/invite/abc"),
            Some(Brand::Arbos)
        );
        assert_eq!(
            Brand::for_url("https://jacob.arbos.life/app/"),
            Some(Brand::Arbos)
        );
        assert_eq!(
            Brand::for_url("https://maps.app.goo.gl/abc"),
            Some(Brand::GoogleMaps)
        );
        assert_eq!(
            Brand::for_url("https://www.google.com/maps/place/Lima"),
            Some(Brand::GoogleMaps)
        );
        assert_eq!(Brand::for_url("https://www.google.com/search?q=x"), None);
        assert_eq!(Brand::for_url("GitHub.com"), Some(Brand::GitHub));
        assert_eq!(Brand::for_host("www.github.com"), Some(Brand::GitHub));
    }

    #[test]
    fn the_google_editors_are_told_apart_by_path() {
        assert_eq!(
            Brand::for_url("https://docs.google.com/document/d/1/edit"),
            Some(Brand::GoogleDocs)
        );
        assert_eq!(
            Brand::for_url("https://docs.google.com/spreadsheets/d/1/edit"),
            Some(Brand::GoogleSheets)
        );
        assert_eq!(
            Brand::for_url("https://docs.google.com/presentation/d/1/edit"),
            Some(Brand::GoogleSlides)
        );
        assert_eq!(Brand::for_host("docs.google.com"), Some(Brand::GoogleDocs));
    }

    #[test]
    fn unknown_hosts_have_no_brand() {
        for url in [
            "https://nypost.com/2026/10/27/story",
            "https://example.com",
            "https://githubusercontent.evil.com/x",
            "https://notgithub.com",
            "https://x.company.com",
            "mailto:a@b.com",
            "",
        ] {
            assert_eq!(Brand::for_url(url), None, "{url}");
        }
    }

    #[test]
    fn every_brand_has_an_asset_that_is_one_filled_svg_path() {
        for b in BRANDS {
            let svg = b.svg();
            assert!(
                svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""),
                "{b:?}"
            );
            assert_eq!(
                svg.matches("<path ").count(),
                if b == Brand::Arbos { 2 } else { 1 },
                "{b:?}"
            );
            assert!(
                svg.contains("fill=\"#000\""),
                "{b:?} paints a fill the renderer tints"
            );
            assert!(
                !svg.contains("stroke="),
                "{b:?} is a filled mark, not strokes"
            );
            assert_eq!(brand_asset_bytes(b.asset()), Some(svg.as_bytes()), "{b:?}");
            assert!(
                b.source().starts_with("http") || b == Brand::Arbos,
                "{b:?} names its source"
            );
        }
        assert_eq!(brand_asset_bytes("brands/nope.svg"), None);
    }

    #[test]
    fn black_and_white_brands_are_mono_and_the_rest_keep_their_colour() {
        assert_eq!(Brand::GitHub.ink(), Ink::Mono);
        assert_eq!(Brand::X.ink(), Ink::Mono);
        assert_eq!(Brand::Notion.ink(), Ink::Mono);
        assert_eq!(Brand::Wikipedia.ink(), Ink::Mono);
        assert_eq!(Brand::Arbos.ink(), Ink::Mono);
        assert_eq!(Brand::Spotify.ink(), Ink::Color(0x1ED760));
        assert_eq!(Brand::YouTube.ink(), Ink::Color(0xFF0000));
    }

    #[test]
    fn metrics_follow_the_pane_kit() {
        assert_eq!(radius(36.0), 10.0, "pane row tile = kit corner");
        assert_eq!(radius(32.0), 9.0, "library thumb");
        assert_eq!(radius(14.0), 4.0, "inline icon = quarter");
        assert_eq!((36.0 * MARK_RATIO).round(), 20.0);
    }

    #[test]
    fn initials_are_the_hosts_first_letter() {
        assert_eq!(initial("nypost.com"), "N");
        assert_eq!(initial("www.elvirrey.com"), "E");
        assert_eq!(initial("3dprint.io"), "3");
        assert_eq!(initial(""), "#");
        assert_eq!(initial("-.-"), "#");
    }

    fn rgba(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)))
    }

    #[test]
    fn tones_tell_a_dark_mark_from_a_light_one_and_an_opaque_icon() {
        // The GitHub favicon shape: black octocat on a transparent field.
        let octocat = rgba(32, 32, |x, y| {
            let inside = (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) < 120;
            if inside {
                [20, 20, 20, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(tone_of(&octocat), Tone::DarkMark);
        let white_mark = rgba(32, 32, |x, _| {
            if x < 20 {
                [250, 250, 250, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(tone_of(&white_mark), Tone::LightMark);
        // A full-bleed apple-touch-icon: opaque, whatever its colour.
        let opaque = rgba(32, 32, |_, _| [10, 10, 10, 255]);
        assert_eq!(tone_of(&opaque), Tone::Neutral);
        // A green mark on transparency is mid-tone: the theme's own fill is fine.
        let green = rgba(32, 32, |x, _| {
            if x < 24 {
                [30, 215, 96, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(tone_of(&green), Tone::Neutral);
        // A sliver of transparency (rounded corners) does not make a mark.
        let corners = rgba(32, 32, |x, y| {
            if x + y < 2 {
                [0, 0, 0, 0]
            } else {
                [20, 20, 20, 255]
            }
        });
        assert_eq!(tone_of(&corners), Tone::Neutral);
    }

    /// A dark mark gets a light fill only in the dark theme; a light mark a dark fill only in
    /// the light theme; brands and opaque pictures sit on `surface_2` in both.
    #[gpui::test]
    fn fills_flip_so_a_mark_never_sits_on_its_own_tone(cx: &mut gpui::TestAppContext) {
        use crate::ui::theme::{AppearancePref, Surface};
        let image = Arc::new(RenderImage::new(Vec::new()));
        for (pref, light) in [(AppearancePref::Dark, false), (AppearancePref::Light, true)] {
            let theme = cx.update(|cx| Theme::for_pref(pref, Surface::Matte, cx));
            assert_eq!(theme.is_light(), light);
            let dark_mark = fill(&Icon::Picture(&image, Tone::DarkMark), &theme);
            let light_mark = fill(&Icon::Picture(&image, Tone::LightMark), &theme);
            let brand = fill(&Icon::Brand(Brand::GitHub), &theme);
            let opaque = fill(&Icon::Picture(&image, Tone::Neutral), &theme);
            assert_eq!(brand, theme.surface_2);
            assert_eq!(opaque, theme.surface_2);
            if light {
                assert_eq!(dark_mark, theme.surface_2);
                assert_eq!(light_mark, dark_fill());
                assert!(
                    Brand::GitHub.color(&theme).l < 0.5,
                    "GitHub is near-black in the light"
                );
            } else {
                assert_eq!(dark_mark, light_fill());
                assert_eq!(light_mark, theme.surface_2);
                assert!(
                    Brand::GitHub.color(&theme).l > 0.8,
                    "GitHub is white in the dark"
                );
            }
            // Spotify keeps its green on both.
            let g: Hsla = gpui::rgb(0x1ED760).into();
            assert_eq!(Brand::Spotify.color(&theme), g);
        }
    }

    /// One brand tile and one letter tile at `size`, side by side.
    struct Tiles(f32, Brand);

    impl gpui::Render for Tiles {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            div()
                .flex()
                .gap(px(8.0))
                .p(px(8.0))
                .child(
                    tile(Icon::Brand(self.1), self.0, &theme)
                        .debug_selector(|| "tile-brand".into()),
                )
                .child(
                    tile(Icon::Initial("nypost.com".into()), self.0, &theme)
                        .debug_selector(|| "tile-initial".into()),
                )
        }
    }

    /// Headless GPUI render: a 36 pt tile is 36 × 36, its brand mark 20 × 20 centred (8 pt of
    /// clear space each side); a 32 pt tile's mark is 18 with 7; the letter tile is the same
    /// square as the brand's.
    #[gpui::test]
    fn the_tile_is_square_and_the_mark_is_centred_with_clear_space(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let theme = Theme::for_pref(
                crate::ui::theme::AppearancePref::Dark,
                crate::ui::theme::Surface::Matte,
                cx,
            );
            cx.set_global(theme);
        });
        let f = |p: gpui::Pixels| -> f32 { p.into() };
        for (size, brand, mark_px, space) in [
            (36.0, Brand::GitHub, 20.0, 8.0),
            (32.0, Brand::Spotify, 18.0, 7.0),
        ] {
            let (_view, vcx) = cx.add_window_view(|_, _| Tiles(size, brand));
            vcx.run_until_parked();
            let tile = vcx.debug_bounds("tile-brand").expect("brand tile");
            assert_eq!(
                (f(tile.size.width), f(tile.size.height)),
                (size, size),
                "{brand:?}"
            );
            let mark = vcx.debug_bounds(MARK_SELECTOR).expect("mark");
            assert_eq!(
                (f(mark.size.width), f(mark.size.height)),
                (mark_px, mark_px)
            );
            assert_eq!(
                f(mark.origin.x) - f(tile.origin.x),
                space,
                "clear space left"
            );
            assert_eq!(
                f(mark.origin.y) - f(tile.origin.y),
                space,
                "clear space top"
            );
            let initial = vcx.debug_bounds("tile-initial").expect("letter tile");
            assert_eq!(
                (f(initial.size.width), f(initial.size.height)),
                (size, size)
            );
        }
    }

    #[test]
    fn icon_for_prefers_the_brand_then_the_picture_then_the_letter() {
        let image = Arc::new(RenderImage::new(Vec::new()));
        assert!(matches!(
            icon_for(
                "https://github.com/a",
                "github.com",
                Some((&image, Tone::Neutral))
            ),
            Icon::Brand(Brand::GitHub)
        ));
        assert!(matches!(
            icon_for(
                "https://nypost.com/a",
                "nypost.com",
                Some((&image, Tone::DarkMark))
            ),
            Icon::Picture(_, Tone::DarkMark)
        ));
        assert!(matches!(
            icon_for("https://nypost.com/a", "nypost.com", None),
            Icon::Initial(h) if h == "nypost.com"
        ));
    }
}
