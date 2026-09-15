//! The app's own UI face, shipped inside the binary.
//!
//! Cursor ships its font; so does Arbos. Asking the system for `sans-serif`
//! gave a different face on every machine — SF on a Mac, whatever
//! fontconfig had on Linux — and a Linux box with no semibold face drew the
//! weights the theme asks for at regular, so a half-bold paragraph looked
//! plain there and was only ever seen on a Mac (symmetry ledger F-53).
//!
//! Inter 4.1, the static faces the theme reaches for — the four weights
//! ([`FontWeight::NORMAL`] through [`FontWeight::BOLD`]) upright and italic —
//! under the SIL Open Font License 1.1, whose text ships beside them in
//! `fonts/inter/LICENSE.txt`. Source: <https://github.com/rsms/inter>,
//! release v4.1, `extras/ttf/`.

use bezel::gpui::App;
use std::borrow::Cow;

/// The family name the theme asks for, as the faces' `name` table spells it.
pub const FAMILY: &str = "Inter";

static FACES: [&[u8]; 8] = [
    include_bytes!("../fonts/inter/Inter-Regular.ttf"),
    include_bytes!("../fonts/inter/Inter-Medium.ttf"),
    include_bytes!("../fonts/inter/Inter-SemiBold.ttf"),
    include_bytes!("../fonts/inter/Inter-Bold.ttf"),
    include_bytes!("../fonts/inter/Inter-Italic.ttf"),
    include_bytes!("../fonts/inter/Inter-MediumItalic.ttf"),
    include_bytes!("../fonts/inter/Inter-SemiBoldItalic.ttf"),
    include_bytes!("../fonts/inter/Inter-BoldItalic.ttf"),
];

/// Register the bundled faces with the text system. Before the first
/// window: the theme names the family, and a face registered after a
/// layout was shaped would not reach it.
pub fn register(cx: &App) -> bezel::gpui::Result<()> {
    cx.text_system()
        .add_fonts(FACES.iter().map(|face| Cow::Borrowed(*face)).collect())
}
