//! Project avatars: a slug from the tree manifest the apps ship. New projects get a
//! deterministic default so every client and the relay agree without a round trip:
//! `slugs[fnv1a64(project_id) % slugs.len()]`, over the manifest in its listed order.
//!
//! The manifest reaches the relay through `ARBOS_AVATAR_MANIFEST` (a JSON file: `["oak", …]`,
//! `{"slugs": […]}`, or `{"avatars": [{"slug": …}, …]}`) or `ARBOS_AVATAR_SLUGS` (comma list);
//! without either, the built-in list below is used; it equals the apps' manifest (2026-10-15),
//! in manifest order.

use arbos_core::avatar::leaf;
use serde_json::{json, Value};

pub const DEFAULT_SLUGS: &[&str] = &[
    "oak", "pine", "lollipop", "fractal", "mushroom", "cypress", "willow", "palm", "bonsai",
    "birch", "baobab", "sapling", "maple", "acacia", "spruce", "cherry", "cactus", "sequoia",
    "olive", "ginkgo",
];

/// Avatar colour palette (manifest `colors[]`), in order. `mono` stays selectable but is
/// never a default: coloured trees are the agent look, the white tree is the brand mark (D74).
pub const DEFAULT_COLORS: &[&str] = &[
    "mono", "red", "orange", "amber", "green", "teal", "cyan", "blue", "indigo", "violet", "pink",
    "rose", "slate",
];

/// The colour slug that is never picked by default.
pub const MONO: &str = "mono";

/// Telegram-style coloured initials (D79): one shared 7-entry palette for every client, each a
/// top → bottom gradient, with a dark-mode pair (same hue, lightness −7 %, so white text keeps
/// ≥ 4.5:1). Index order is part of the contract.
pub const INITIALS_PALETTE: [InitialsColor; 7] = [
    InitialsColor {
        name: "red",
        light: ("#FF845E", "#D45246"),
        dark: ("#FF693A", "#C83B2E"),
    },
    InitialsColor {
        name: "orange",
        light: ("#FEBB5B", "#F68136"),
        dark: ("#FEAC38", "#F46C14"),
    },
    InitialsColor {
        name: "violet",
        light: ("#B694F9", "#6C61DF"),
        dark: ("#9F72F7", "#5043D9"),
    },
    InitialsColor {
        name: "green",
        light: ("#9AD164", "#46BA43"),
        dark: ("#88C948", "#3CA03A"),
    },
    InitialsColor {
        name: "cyan",
        light: ("#53EDD6", "#28C9B7"),
        dark: ("#33EACE", "#22AB9C"),
    },
    InitialsColor {
        name: "blue",
        light: ("#5CAFFA", "#408ACF"),
        dark: ("#399EF9", "#2F78BC"),
    },
    InitialsColor {
        name: "pink",
        light: ("#FF8AA5", "#E05F8E"),
        dark: ("#FF668A", "#DA4179"),
    },
];

#[derive(Debug, Clone, Copy)]
pub struct InitialsColor {
    pub name: &'static str,
    /// (top, bottom) in light mode.
    pub light: (&'static str, &'static str),
    /// (top, bottom) in dark mode.
    pub dark: (&'static str, &'static str),
}

/// Which palette entry a stable id gets: `(sum of the id's UTF-8 bytes) mod 7`. Stable id =
/// project id for agents, user id for people. Every client implements exactly this.
pub fn color_index(stable_id: &str) -> u8 {
    (stable_id.bytes().map(u64::from).sum::<u64>() % 7) as u8
}

/// The index an agent renders with: the owner's override when set, else the derived one.
pub fn effective_color_index(stable_id: &str, override_index: Option<i64>) -> u8 {
    match override_index {
        Some(i) if (0..7).contains(&i) => i as u8,
        _ => color_index(stable_id),
    }
}

/// Leaf avatar (D92) in effect for a project: the owner's picks where valid, else the id's cell.
pub fn effective_leaf(p: &crate::db::Project) -> leaf::LeafPick {
    leaf::pick_for(
        &p.id,
        p.avatar_leaf.and_then(|v| u8::try_from(v).ok()),
        p.avatar_leaf_color.and_then(|v| u8::try_from(v).ok()),
    )
}

/// Validate `avatar_leaf` / `avatar_leaf_color` from a request body. Besides the leaf and
/// hue indices, both accept [`leaf::NEUTRAL`] (255) — "no leaf": the Create sheets store it
/// when nothing was uploaded or generated, and the clients draw the name's initial on a
/// neutral disc (Jacob, 2026-10-25). `null` stays the automatic leaf.
pub fn check_leaf_pick(
    leaf_idx: Option<i64>,
    color_idx: Option<i64>,
) -> Result<(), (axum::http::StatusCode, String)> {
    let neutral = i64::from(leaf::NEUTRAL);
    if let Some(l) = leaf_idx {
        if !(0..i64::from(leaf::LEAF_COUNT)).contains(&l) && l != neutral {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "avatar_leaf must be 0–{}, {neutral} (no leaf) or null",
                    leaf::LEAF_COUNT - 1
                ),
            ));
        }
    }
    if let Some(c) = color_idx {
        if !(0..i64::from(leaf::COLOR_COUNT)).contains(&c) && c != neutral {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "avatar_leaf_color must be 0–{}, {neutral} (no leaf) or null",
                    leaf::COLOR_COUNT - 1
                ),
            ));
        }
    }
    Ok(())
}

/// `GET /avatar-leaves` body: the D92 (v2) shapes, hues and index rule, so a site or a third
/// client can draw the same leaf — a pastel disc (`hex`) with the darker same-hue leaf (`ink`).
pub fn leaves_json() -> Value {
    json!({
        "rule": {
            "version": leaf::LEAF_RULE_VERSION,
            "hash": "fnv1a64(project_id)",
            "leaf": "hash mod 16",
            "color": "the leaf's default_color",
            "render": "disc = colors[color].hex; leaf = colors[color].ink, path drawn edge to edge (it spans 71 of 100 units, centred), even-odd fill",
            "override": "avatar_leaf 0–15, avatar_leaf_color 0–9 on PATCH /projects/{id}; null = automatic",
            "neutral": "avatar_leaf = avatar_leaf_color = 255: no leaf — draw the name's first letter on a neutral disc (dark #3A3A3A / #E5E5E5, light #E6E6E6 / #4A4A4A); never an automatic leaf",
        },
        "scale": leaf::LEAF_SCALE,
        "leaves": leaf::LEAVES.iter().enumerate().map(|(i, l)| json!({
            "index": i, "slug": l.slug, "name": l.name, "path": leaf::path_data(i as u8),
            "pivot": [l.pivot.0, l.pivot.1], "idle_secs": l.idle_secs,
            "default_color": l.default_color,
        })).collect::<Vec<_>>(),
        "colors": leaf::COLORS.iter().enumerate().map(|(i, c)| json!({
            "index": i, "name": c.name, "hex": c.rgb.hex(), "ink": c.ink.hex(),
        })).collect::<Vec<_>>(),
    })
}

/// `GET /avatar-colors` body.
pub fn palette_json() -> Value {
    serde_json::json!({
        "derivation": "sum_utf8_bytes_mod_7",
        "palette": INITIALS_PALETTE.iter().enumerate().map(|(i, c)| serde_json::json!({
            "index": i, "name": c.name,
            "light": { "top": c.light.0, "bottom": c.light.1 },
            "dark": { "top": c.dark.0, "bottom": c.dark.1 },
        })).collect::<Vec<_>>(),
    })
}

/// The palette minus `mono`, for default picks; `mono` is kept only if it is all there is.
fn default_palette(colors: &[String]) -> Vec<String> {
    let all: Vec<String> = if colors.is_empty() {
        DEFAULT_COLORS.iter().map(|s| (*s).to_owned()).collect()
    } else {
        colors.to_vec()
    };
    let coloured: Vec<String> = all
        .iter()
        .filter(|c| !c.eq_ignore_ascii_case(MONO))
        .cloned()
        .collect();
    if coloured.is_empty() {
        all
    } else {
        coloured
    }
}

/// FNV-1a 64 over the UTF-8 bytes (the same arithmetic is one line in Swift).
pub fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn default_for(project_id: &str, slugs: &[String]) -> String {
    if slugs.is_empty() {
        return DEFAULT_SLUGS[(fnv1a64(project_id) % DEFAULT_SLUGS.len() as u64) as usize]
            .to_owned();
    }
    slugs[(fnv1a64(project_id) % slugs.len() as u64) as usize].clone()
}

/// Default colour for a new project: `coloured[fnv1a64(project_id + ":color") % n]` over the
/// palette without `mono` (D74: a new agent is always a coloured tree).
pub fn default_color_for(project_id: &str, colors: &[String]) -> String {
    let list = default_palette(colors);
    list[(fnv1a64(&format!("{project_id}:color")) % list.len() as u64) as usize].clone()
}

/// Default `(tree, color)` for a new project, deduped against the user's existing pairs: start
/// at the hash picks; probe forward on the colour first (same tree, next colour), then on the
/// tree, until the pair is unused; when every pair is taken, keep the hash picks. Two agents may
/// share a tree only when their colours differ.
pub fn default_pair(
    project_id: &str,
    slugs: &[String],
    colors: &[String],
    taken: &[(String, String)],
) -> (String, String) {
    let trees: Vec<String> = if slugs.is_empty() {
        DEFAULT_SLUGS.iter().map(|s| (*s).to_owned()).collect()
    } else {
        slugs.to_vec()
    };
    let palette = default_palette(colors);
    let t0 = (fnv1a64(project_id) % trees.len() as u64) as usize;
    let c0 = (fnv1a64(&format!("{project_id}:color")) % palette.len() as u64) as usize;
    for ti in 0..trees.len() {
        let tree = &trees[(t0 + ti) % trees.len()];
        for ci in 0..palette.len() {
            let color = &palette[(c0 + ci) % palette.len()];
            if !taken.iter().any(|(t, c)| t == tree && c == color) {
                return (tree.clone(), color.clone());
            }
        }
    }
    (trees[t0].clone(), palette[c0].clone())
}

/// Colour list from the manifest file (`colors[]`), `ARBOS_AVATAR_COLORS`, else the default.
pub fn colors_from_env() -> Vec<String> {
    if let Ok(path) = std::env::var("ARBOS_AVATAR_MANIFEST") {
        if let Ok(text) = std::fs::read_to_string(path.trim()) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let colors: Vec<String> = v
                    .get("colors")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| {
                                e.as_str().map(str::to_owned).or_else(|| {
                                    e.get("slug").and_then(Value::as_str).map(str::to_owned)
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !colors.is_empty() {
                    return colors;
                }
            }
        }
    }
    if let Ok(list) = std::env::var("ARBOS_AVATAR_COLORS") {
        let colors: Vec<String> = list
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if !colors.is_empty() {
            return colors;
        }
    }
    DEFAULT_COLORS.iter().map(|s| (*s).to_owned()).collect()
}

/// Slug list from the env (file or list), else the built-in default.
pub fn slugs_from_env() -> Vec<String> {
    if let Ok(path) = std::env::var("ARBOS_AVATAR_MANIFEST") {
        if let Ok(text) = std::fs::read_to_string(path.trim()) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let list = match &v {
                    Value::Array(a) => a.clone(),
                    Value::Object(o) => o
                        .get("slugs")
                        .or_else(|| o.get("avatars"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let slugs: Vec<String> = list
                    .iter()
                    .filter_map(|e| {
                        e.as_str()
                            .map(str::to_owned)
                            .or_else(|| e.get("slug").and_then(Value::as_str).map(str::to_owned))
                    })
                    .collect();
                if !slugs.is_empty() {
                    return slugs;
                }
            }
        }
    }
    if let Ok(list) = std::env::var("ARBOS_AVATAR_SLUGS") {
        let slugs: Vec<String> = list
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if !slugs.is_empty() {
            return slugs;
        }
    }
    DEFAULT_SLUGS.iter().map(|s| (*s).to_owned()).collect()
}

/// A slug a client may set: short, lower-case, letters/digits/hyphens.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

#[cfg(test)]
mod initials_tests {
    //! D79 derivation is byte arithmetic every client can reproduce.

    use super::*;

    #[test]
    fn index_is_byte_sum_mod_7() {
        // "gh_1": 103 + 104 + 95 + 49 = 351; 351 mod 7 = 1.
        assert_eq!(color_index("gh_1"), 1);
        // "u_mac": 117 + 95 + 109 + 97 + 99 = 517; 517 mod 7 = 6.
        assert_eq!(color_index("u_mac"), 6);
        assert_eq!(color_index(""), 0);
        assert!(color_index("01M51AF8N6QFBDW8H9Y76W82T0") < 7);
        assert_eq!(effective_color_index("gh_1", Some(4)), 4);
        assert_eq!(
            effective_color_index("gh_1", Some(9)),
            1,
            "out-of-range overrides fall back"
        );
        assert_eq!(effective_color_index("gh_1", None), 1);
        let p = palette_json();
        assert_eq!(p["palette"].as_array().unwrap().len(), 7);
        assert_eq!(p["palette"][2]["name"], "violet");
        assert_eq!(p["palette"][0]["light"]["top"], "#FF845E");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Create sheets' "nothing picked" value (Jacob, 2026-10-25) is accepted on both
    /// fields; the ranges and `null` are unchanged, and anything else is still a 400.
    #[test]
    fn neutral_pick_is_accepted_and_the_rest_still_validates() {
        let n = i64::from(leaf::NEUTRAL);
        assert!(check_leaf_pick(Some(n), Some(n)).is_ok());
        assert!(check_leaf_pick(Some(n), None).is_ok());
        assert!(check_leaf_pick(None, None).is_ok());
        assert!(check_leaf_pick(Some(15), Some(9)).is_ok());
        assert!(check_leaf_pick(Some(16), None).is_err());
        assert!(check_leaf_pick(None, Some(10)).is_err());
        assert!(check_leaf_pick(Some(254), None).is_err());
        assert!(check_leaf_pick(Some(-1), None).is_err());
        // The effective (automatic) leaf an older client falls back to stays in range.
        let p: crate::db::Project = serde_json::from_value(json!({
            "id": "01M5NEUTRAL", "name": "Field notes", "icon": "", "instructions": "",
            "models": {}, "state": "ready", "created_at": 0, "last_active_at": 0,
            "spend_cap_daily": 1.0, "color": "", "user_id": "u", "workspace_state": "stopped",
            "avatar_leaf": n, "avatar_leaf_color": n,
        }))
        .unwrap();
        let e = effective_leaf(&p);
        assert!(e.leaf < leaf::LEAF_COUNT && e.color < leaf::COLOR_COUNT);
        assert!(leaves_json()["rule"]["neutral"]
            .as_str()
            .unwrap()
            .contains("255"));
    }

    #[test]
    fn default_pair_dedupes_on_color_first() {
        let trees: Vec<String> = DEFAULT_SLUGS.iter().map(|s| (*s).to_owned()).collect();
        let colors: Vec<String> = DEFAULT_COLORS.iter().map(|s| (*s).to_owned()).collect();
        let (t, c) = default_pair("P9", &trees, &colors, &[]);
        assert_eq!(t, default_for("P9", &trees));
        assert_eq!(c, default_color_for("P9", &colors));
        // Same tree taken with that colour: the tree stays, the colour moves on.
        let (t2, c2) = default_pair("P9", &trees, &colors, &[(t.clone(), c.clone())]);
        assert_eq!(t2, t);
        let coloured = default_palette(&colors);
        let ci = coloured.iter().position(|x| *x == c).unwrap();
        assert_eq!(c2, coloured[(ci + 1) % coloured.len()]);
        // Every colour of that tree taken: the next tree.
        let all: Vec<(String, String)> = coloured.iter().map(|c| (t.clone(), c.clone())).collect();
        let (t3, _) = default_pair("P9", &trees, &colors, &all);
        let ti = trees.iter().position(|x| *x == t).unwrap();
        assert_eq!(t3, trees[(ti + 1) % trees.len()]);
    }

    #[test]
    fn defaults_are_never_mono() {
        let trees: Vec<String> = DEFAULT_SLUGS.iter().map(|s| (*s).to_owned()).collect();
        let colors: Vec<String> = DEFAULT_COLORS.iter().map(|s| (*s).to_owned()).collect();
        for i in 0..500 {
            let id = format!("01HPROJECT{i:04}");
            assert_ne!(default_color_for(&id, &colors), MONO);
            assert_ne!(default_color_for(&id, &[]), MONO);
            let (_, c) = default_pair(&id, &trees, &colors, &[]);
            assert_ne!(c, MONO);
        }
        // A palette that is only mono still yields something.
        assert_eq!(default_color_for("x", &[MONO.to_owned()]), MONO);
        // Case-insensitive exclusion.
        assert_eq!(
            default_color_for("x", &["Mono".to_owned(), "red".to_owned()]),
            "red"
        );
    }

    #[test]
    fn default_is_deterministic_and_in_range() {
        let slugs: Vec<String> = DEFAULT_SLUGS.iter().map(|s| (*s).to_owned()).collect();
        let a = default_for("01HPROJECT", &slugs);
        assert_eq!(a, default_for("01HPROJECT", &slugs));
        assert!(slugs.contains(&a));
        // Reference values for the apps' implementation of the same rule (FNV-1a 64).
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
