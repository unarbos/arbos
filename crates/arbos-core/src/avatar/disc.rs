//! Solid-colour agent discs (D92, amended 2026-11-05): the default picture of an agent that has
//! no uploaded or generated photo. **A plain filled circle in one of ten colours** — no glyph,
//! no artwork. People keep their D79 initials.
//!
//! * The colour is deterministic from the agent id: `colour = fnv1a64(id) mod 10`. The Mac, the
//!   relay and — through arbos-ffi — the iPhone all read this one table, so an agent is the same
//!   circle on every client and on the next launch, with no round trip.
//! * The owner may pin a colour (`avatar_disc_color` on the relay, `null` = automatic).
//! * The disc carries no letter. The single exception is [`NEUTRAL`], the stored "no picture"
//!   choice, which draws the name's first letter on a neutral grey disc.

use super::{Accent, Rgb};

/// Number of disc colours.
pub const COLOR_COUNT: u8 = 10;

/// Bump when the index rule changes, so clients refresh cached defaults. 1 and 2 were the leaf
/// avatars this replaced; a client that cached a leaf index sees a new version and re-derives.
pub const DISC_RULE_VERSION: u32 = 3;

/// "No picture" (Jacob, 2026-10-25): an agent, group or server created from the Create sheet
/// without an uploaded or generated picture is stored with `avatar_disc_color` = `NEUTRAL` and
/// draws as **the name's first letter on a neutral grey disc** — never an automatic colour.
/// Distinct from `null` (= the id's automatic colour), so a deliberate "nothing picked" survives.
/// The relay accepts the value; [`color_for`] ignores it, so an older client draws the automatic
/// disc instead of failing.
pub const NEUTRAL: u8 = 255;
/// Neutral disc fill and the letter's ink: dark appearance `(disc, ink)`, light appearance
/// `(disc, ink)`. The picker's own "unselected" grey, so the sheets and the lists agree.
pub const NEUTRAL_DARK: (Rgb, Rgb) = (Rgb(0x3A, 0x3A, 0x3A), Rgb(0xE5, 0xE5, 0xE5));
pub const NEUTRAL_LIGHT: (Rgb, Rgb) = (Rgb(0xE6, 0xE6, 0xE6), Rgb(0x4A, 0x4A, 0x4A));

/// Is this stored pick the neutral one (no picture: initial on a grey disc)?
pub fn is_neutral(color: Option<u8>) -> bool {
    color == Some(NEUTRAL)
}

/// The one letter a neutral disc carries: the first letter or digit of the name, upper-cased;
/// `?` for an empty name. One letter, not D79's two — Jacob's "the name's initial".
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".to_owned())
}

/// The bubble-bar accent of a neutral agent: a mid grey in both appearances.
pub fn neutral_accent() -> Accent {
    Accent {
        dark: Rgb(0x8A, 0x8A, 0x8A),
        light: Rgb(0x6E, 0x6E, 0x6E),
    }
}

/// One disc colour: a solid fill that white text and the bubble bar both read against.
#[derive(Clone, Copy, Debug)]
pub struct DiscColor {
    pub name: &'static str,
    pub rgb: Rgb,
}

/// The ten colours, index order. The order is part of the contract: a stored pick is an index.
pub const COLORS: [DiscColor; COLOR_COUNT as usize] = [
    DiscColor {
        name: "Coral",
        rgb: Rgb(0xE0, 0x63, 0x4F),
    },
    DiscColor {
        name: "Royal blue",
        rgb: Rgb(0x2F, 0x5B, 0xC7),
    },
    DiscColor {
        name: "Green",
        rgb: Rgb(0x3E, 0x9A, 0x5A),
    },
    DiscColor {
        name: "Violet",
        rgb: Rgb(0x7B, 0x57, 0xC9),
    },
    DiscColor {
        name: "Teal",
        rgb: Rgb(0x2A, 0x9C, 0x8C),
    },
    DiscColor {
        name: "Pink",
        rgb: Rgb(0xD9, 0x55, 0x8A),
    },
    DiscColor {
        name: "Yellow",
        rgb: Rgb(0xD9, 0xA4, 0x1E),
    },
    DiscColor {
        name: "Orange",
        rgb: Rgb(0xE4, 0x80, 0x2B),
    },
    DiscColor {
        name: "Lime",
        rgb: Rgb(0x88, 0xB4, 0x25),
    },
    DiscColor {
        name: "Deep purple",
        rgb: Rgb(0x5E, 0x3E, 0x9E),
    },
];

/// FNV-1a over the UTF-8 bytes, 64-bit.
pub fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The automatic colour for an agent id: `fnv1a64(id) mod 10`.
pub fn default_color(seed: &str) -> u8 {
    (fnv1a64(seed) % u64::from(COLOR_COUNT)) as u8
}

/// The colour in effect: the owner's pick where valid, else the automatic one.
pub fn color_for(seed: &str, color: Option<u8>) -> u8 {
    color
        .filter(|c| *c < COLOR_COUNT)
        .unwrap_or_else(|| default_color(seed))
}

/// The disc fill of colour `index`.
pub fn fill(index: u8) -> Rgb {
    COLORS[(index % COLOR_COUNT) as usize].rgb
}

/// The bubble-bar accent of an agent drawing its disc: the disc colour itself, both appearances.
pub fn accent(index: u8) -> Accent {
    let rgb = fill(index);
    Accent {
        dark: rgb,
        light: rgb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_colour_is_stable_and_spread() {
        assert_eq!(default_color("x"), default_color("x"));
        let mut seen = std::collections::HashSet::new();
        for i in 0..2000 {
            let c = default_color(&format!("01M50W7HPM0KCFVMQ32HXN{i:04}"));
            assert!(c < COLOR_COUNT);
            seen.insert(c);
        }
        assert_eq!(
            seen.len(),
            COLOR_COUNT as usize,
            "2000 ids should reach every colour"
        );
        // Reference values other clients can check against.
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        let id = "01M50W7HPM0KCFVMQ32HXNTN5S";
        assert_eq!(default_color(id), (fnv1a64(id) % 10) as u8);
    }

    #[test]
    fn overrides_apply_when_valid() {
        assert_eq!(color_for("seed", None), default_color("seed"));
        assert_eq!(color_for("seed", Some(7)), 7);
        // Out of range falls back rather than panicking on an index from an older client.
        assert_eq!(color_for("seed", Some(10)), default_color("seed"));
        assert_eq!(color_for("seed", Some(NEUTRAL)), default_color("seed"));
    }

    /// Jacob, 2026-10-25: the Create sheets' fallback is the name's initial on a neutral disc,
    /// stored as `NEUTRAL` so it is told apart from `null` (the automatic colour).
    #[test]
    fn neutral_is_its_own_value_and_old_clients_fall_back() {
        assert!(is_neutral(Some(NEUTRAL)));
        assert!(!is_neutral(None));
        assert!(!is_neutral(Some(0)));
        assert_eq!(initial("Field notes"), "F");
        assert_eq!(initial("  @ops-bot"), "O");
        assert_eq!(initial("élan"), "É");
        assert_eq!(initial("42 things"), "4");
        assert_eq!(initial(""), "?");
        assert_eq!(initial("—"), "?");
    }

    #[test]
    fn every_colour_is_a_solid_opaque_fill() {
        let mut hexes = std::collections::HashSet::new();
        for (i, c) in COLORS.iter().enumerate() {
            assert!(!c.name.is_empty());
            // Distinct, and never so pale that white text on it would fail.
            assert!(hexes.insert(c.rgb.hex()), "{} repeats a colour", c.name);
            let Rgb(r, g, b) = c.rgb;
            let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
            assert!(lum < 200.0, "{} is too light for white text", c.name);
            assert_eq!(fill(i as u8).hex(), c.rgb.hex());
            // The accent is the fill, in both appearances.
            assert_eq!(accent(i as u8).dark, c.rgb);
            assert_eq!(accent(i as u8).light, c.rgb);
        }
        // Indices wrap rather than panic.
        assert_eq!(fill(COLOR_COUNT).hex(), COLORS[0].rgb.hex());
    }
}
