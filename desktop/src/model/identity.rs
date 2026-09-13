//! A project's face: the name its tab shows, the glyph beside it, and the
//! colour that glyph takes — Cursor's Projects list, one row per project.
//!
//! Filed at `<project>/.arbos/project.toml` so the folder carries it:
//! opening the folder again brings the same face back, and any other client
//! (the phone's panel) reads the same three fields.

use crate::{assets, model::place::Place};
use bezel::{gpui::Hsla, ui::icons};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file, under `.arbos/`.
const FILE: &str = "project.toml";

/// The glyphs a tab may wear, by the name the file stores. Bezel's set,
/// plus the house the home tab wears.
pub const GLYPHS: &[(&str, &str)] = &[
    ("folder", icons::files::FOLDER),
    ("home", assets::HOME_ICON),
    ("star", icons::status::STAR),
    ("book", icons::files::BOOK),
    ("tag", icons::files::TAG),
    ("globe", icons::devices::GLOBAL),
    ("cpu", icons::devices::CPU),
    ("terminal", icons::devices::TERMINAL),
    ("chat", icons::system::CHAT_ROUND_LINE),
    ("checklist", icons::editing::CHECKLIST),
    ("branch", icons::editing::GIT_BRANCH),
    ("grid", icons::system::WIDGET),
];

/// The palette, by the name the file stores. Cursor's Projects icons sit
/// in this range: saturated enough to tell apart, quiet enough to live in
/// the chrome on both appearances.
pub const COLORS: &[(&str, u32)] = &[
    ("blue", 0x4C8DFF),
    ("orange", 0xF08A3C),
    ("purple", 0x9B7BFF),
    ("red", 0xE5533D),
    ("green", 0x3DBD6E),
    ("teal", 0x2FB7B0),
    ("pink", 0xE0609E),
    ("yellow", 0xE0B23C),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Identity {
    /// The tab's label. Empty means the folder's own name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// One of [`GLYPHS`] by name.
    pub icon: String,
    /// One of [`COLORS`] by name, or `#rrggbb`.
    pub color: String,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            name: None,
            icon: GLYPHS[0].0.into(),
            color: COLORS[0].0.into(),
        }
    }
}

impl Identity {
    /// Where the file sits for a project whose `.arbos/` is at `store`.
    pub fn path(store: &Path) -> PathBuf {
        store.join(FILE)
    }

    /// The file, if the folder has one.
    pub fn load(store: &Path) -> Option<Self> {
        let body = std::fs::read_to_string(Self::path(store)).ok()?;
        toml::from_str(&body).ok()
    }

    /// What a folder wears before anyone chooses: its own name; the house
    /// for the home tab, a globe for a folder on another machine, a folder
    /// for the rest; and a colour picked by the path so two new tabs do
    /// not come up the same.
    pub fn defaults(place: &Place, home: bool) -> Self {
        let icon = if home {
            "home"
        } else if place.is_remote() {
            "globe"
        } else {
            GLYPHS[0].0
        };
        let color = COLORS[(hash(&place.encode()) % COLORS.len() as u64) as usize].0;
        Self {
            name: None,
            icon: icon.into(),
            color: color.into(),
        }
    }

    /// Write the file. The directory is made if the folder has no
    /// `.arbos/` yet — a freshly opened tab is exactly that case.
    pub fn save(&self, store: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(store)?;
        let body = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(Self::path(store), body)
    }

    /// The asset path of the glyph; a name the palette does not know
    /// falls back to the folder.
    pub fn glyph(&self) -> &'static str {
        GLYPHS
            .iter()
            .find(|(name, _)| *name == self.icon)
            .map(|(_, path)| *path)
            .unwrap_or(GLYPHS[0].1)
    }

    /// The colour, as the palette or a hex triplet names it.
    pub fn hsla(&self) -> Hsla {
        let hex = COLORS
            .iter()
            .find(|(name, _)| *name == self.color)
            .map(|(_, hex)| *hex)
            .or_else(|| u32::from_str_radix(self.color.trim_start_matches('#'), 16).ok())
            .unwrap_or(COLORS[0].1);
        bezel::gpui::rgb(hex).into()
    }

    /// The palette index of this colour, for the sheet's swatches.
    pub fn color_index(&self) -> Option<usize> {
        COLORS.iter().position(|(name, _)| *name == self.color)
    }

    pub fn glyph_index(&self) -> Option<usize> {
        GLYPHS.iter().position(|(name, _)| *name == self.icon)
    }

    /// The name, trimmed, or none.
    pub fn label(&self) -> Option<&str> {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
}

/// Stable across launches — `DefaultHasher` is salted per process.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(1099511628211);
    }
    h
}
