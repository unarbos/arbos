//! Native file-manager and default-application handoff, independent of UI and transport.

use std::path::Path;
use std::process::Command;

/// Reveal the saved file: Finder on macOS, the file manager (or the default app) elsewhere.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let spawned = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    let spawned = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(err) = spawned {
        eprintln!("reveal {}: {err}", path.display());
    }
}

/// Hand a fetched file to macOS: Quick Look (`qlmanage -p`) for documents, the
/// default app for the rest. Videos in the media viewer play inline instead
/// ([feedback app-01M6147S9TPNBQA7KSDEY0K8C4]); this path is for Quick Look
/// and non-mac fallbacks. Elsewhere `xdg-open`.
pub fn open_with_system(path: &Path, quick_look: bool) {
    #[cfg(target_os = "macos")]
    let spawned = if quick_look {
        Command::new("qlmanage")
            .arg("-p")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        Command::new("open").arg(path).spawn()
    };
    #[cfg(not(target_os = "macos"))]
    let spawned = {
        let _ = quick_look;
        Command::new("xdg-open")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    if let Err(err) = spawned {
        eprintln!("open {}: {err}", path.display());
    }
}
