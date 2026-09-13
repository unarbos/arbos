//! What `svg()` and `img()` can reach by name: bezel's shipped icons, plus the
//! agent icons [`crate::agent`] has cached.
//!
//! A cached icon's asset path is its own path on disk, so the fallback is a
//! read rather than a table — there is nothing to keep in sync.

use anyhow::Result;
use bezel::{
    gpui::{AssetSource, SharedString},
    ui::icons,
};
use std::{borrow::Cow, path::PathBuf};

/// The app's own mark, wherever this build keeps it: beside the binary in a
/// bundle, in the source tree under `cargo run`.
///
/// Nothing at all where neither is there. The logo is downloaded by `make
/// icon` and is not in git, so a fresh clone has none to draw until it has
/// been bundled — and whatever shows it has to hold that case rather than
/// stand an empty box where a picture goes.
pub fn mark() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // `…/Arbos.app/Contents/MacOS/arbos` — the Makefile puts a small copy
    // of the logo beside the `.icns` that AppKit reads, because nothing here
    // can paint an `.icns`.
    let bundled = exe.parent()?.parent()?.join("Resources").join("icon.png");
    let source = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png"));
    [bundled, source].into_iter().find(|path| path.is_file())
}

pub const DELEGATE_ICON: &str = "arbos/delegate.svg";
/// House outline for a local project heading. Bezel's set has no home glyph.
pub const HOME_ICON: &str = "arbos/home.svg";
/// Rounded rect with two dots — the remote / Cursor mark.
pub const REMOTE_ICON: &str = "arbos/remote.svg";

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == DELEGATE_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!("view/delegate.svg"))));
        }
        if path == HOME_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!("view/home.svg"))));
        }
        if path == REMOTE_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!("view/remote.svg"))));
        }
        if let Some(bytes) = icons::Assets.load(path)? {
            return Ok(Some(bytes));
        }
        Ok(std::fs::read(path).ok().map(Cow::Owned))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        icons::Assets.list(path)
    }
}
