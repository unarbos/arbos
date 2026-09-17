//! A place inside a cloud-synced folder. Jacob's `~/Documents/Misc/arbos`
//! sits under iCloud "Desktop & Documents" sync: reads of `.arbos/` there
//! block for minutes on dataless items, and a kernel stalls the same way.
//! This finds such folders (macOS file providers: iCloud Drive, and the
//! `~/Library/CloudStorage` providers — Dropbox, OneDrive, Google Drive),
//! and moves the store out of the sync while keeping `.arbos` working as a
//! symlink. iCloud skips anything named `*.nosync`, so the default is to
//! rename `.arbos` to `.arbos.nosync` beside the project and point
//! `.arbos` at it; the other choice is `~/.arbos/stores/<hash>/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Which sync a path sits under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// iCloud Drive, including Desktop & Documents when that is on.
    ICloud,
    /// A `~/Library/CloudStorage/<provider>` folder (Dropbox, OneDrive, …).
    FileProvider(String),
}

impl Sync {
    pub fn label(&self) -> String {
        match self {
            Sync::ICloud => "iCloud Drive".to_string(),
            Sync::FileProvider(p) => p.split('-').next().unwrap_or(p).to_string(),
        }
    }
}

/// Where a store may live once moved out of the sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relocation {
    /// `.arbos.nosync` beside the project, `.arbos` a symlink to it. iCloud
    /// leaves `*.nosync` alone; the folder stays with the project.
    Nosync,
    /// `~/.arbos/stores/<hash of the place path>/`, `.arbos` a symlink.
    Home,
}

/// Whether `path` (any file or folder) is inside a synced folder, by the
/// real path. macOS only; other platforms answer None.
pub fn detect(path: &Path) -> Option<Sync> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    detect_in(&real, &home)
}

/// `detect` with the home folder given: the rule, testable anywhere.
pub fn detect_in(real: &Path, home: &Path) -> Option<Sync> {
    let mobile = home.join("Library").join("Mobile Documents");
    if real.starts_with(&mobile) {
        return Some(Sync::ICloud);
    }
    let storage = home.join("Library").join("CloudStorage");
    if let Ok(rest) = real.strip_prefix(&storage) {
        let provider = rest
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| "file provider".into());
        return Some(Sync::FileProvider(provider));
    }
    None
}

/// Whether the store of `place` is already kept out of the sync: `.arbos`
/// links to a `*.nosync` folder (which the provider skips) or to a folder
/// outside any synced path.
pub fn settled(place: &Path) -> bool {
    match relocated(place) {
        Some(store) => {
            store
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".nosync"))
                || detect(&store).is_none()
        }
        None => false,
    }
}

/// Where the store of `place` really is, when `.arbos` is a symlink we (or
/// a hand) made to keep it out of the sync.
pub fn relocated(place: &Path) -> Option<PathBuf> {
    let link = place.join(".arbos");
    let meta = std::fs::symlink_metadata(&link).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    std::fs::read_link(&link)
        .ok()
        .map(|t| if t.is_absolute() { t } else { place.join(t) })
}

/// The folder a `Relocation::Home` store goes to for `place`.
pub fn home_store_dir(place: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let real = std::fs::canonicalize(place).unwrap_or_else(|_| place.to_path_buf());
    let hash = short_hash(&real.to_string_lossy());
    Some(home.join(".arbos").join("stores").join(hash))
}

fn short_hash(s: &str) -> String {
    // FNV-1a, 64 bits: stable, dependency-free, plenty for a folder name.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Move the store of `place` out of the sync and leave `.arbos` as a
/// symlink to it. Refuses when a kernel is running on the place (its
/// `runtime/kernel.json` pid is alive), when `.arbos` is already a
/// symlink, or when the target exists. Returns the store's new path.
pub fn relocate(place: &Path, how: Relocation) -> Result<PathBuf> {
    let link = place.join(".arbos");
    if let Ok(meta) = std::fs::symlink_metadata(&link)
        && meta.file_type().is_symlink()
    {
        bail!(
            ".arbos is already a symlink (to {}); nothing to move",
            std::fs::read_link(&link)
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
    }
    if let Some(pid) = kernel_pid(place)
        && pid_alive(pid)
    {
        bail!("a kernel (pid {pid}) is running on this place; stop it first");
    }
    let (target, link_text): (PathBuf, PathBuf) = match how {
        Relocation::Nosync => (place.join(".arbos.nosync"), PathBuf::from(".arbos.nosync")),
        Relocation::Home => {
            let dir = home_store_dir(place).context("no HOME to place the store under")?;
            (dir.clone(), dir)
        }
    };
    if target.exists() {
        bail!("{} already exists; move it aside first", target.display());
    }
    if link.exists() {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&link, &target)
            .with_context(|| format!("move .arbos to {}", target.display()))?;
    } else {
        std::fs::create_dir_all(&target)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&link_text, &link)
        .with_context(|| format!("link .arbos -> {}", link_text.display()))?;
    // The moved store is a nested repository under a new name: the project
    // must not record it either (steward's note on #134).
    if how == Relocation::Nosync {
        crate::files::exclude_locally(place, &[".arbos.nosync/"]);
    }
    #[cfg(not(unix))]
    bail!("relocating the store needs symlinks (unix only)");
    Ok(target)
}

/// The advice `check` and the window give, in one place.
pub fn advice(sync: &Sync, place: &Path) -> String {
    format!(
        "{} is inside {} sync; reads of .arbos/ can block for minutes on items the provider has not downloaded, and a kernel stalls with them. Keep the store out of the sync: `arbos-kernel store nosync {}` renames it to .arbos.nosync (which iCloud skips) and leaves .arbos as a symlink (recommended), or `store home` moves it under ~/.arbos/stores/.",
        place.display(),
        sync.label(),
        place.display()
    )
}

fn kernel_pid(place: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(place.join(".arbos/runtime/kernel.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("pid")?.as_u64().map(|p| p as u32)
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks the pid.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icloud_and_cloud_storage_paths_are_seen_and_others_are_not() {
        let home = Path::new("/Users/jacob");
        assert_eq!(
            detect_in(
                Path::new(
                    "/Users/jacob/Library/Mobile Documents/com~apple~CloudDocs/Documents/Misc/arbos"
                ),
                home
            ),
            Some(Sync::ICloud)
        );
        assert_eq!(
            detect_in(
                Path::new("/Users/jacob/Library/CloudStorage/Dropbox-Personal/work/arbos"),
                home
            ),
            Some(Sync::FileProvider("Dropbox-Personal".into()))
        );
        assert_eq!(detect_in(Path::new("/Users/jacob/code/arbos"), home), None);
        assert_eq!(detect_in(Path::new("/tmp/x"), home), None);
        assert_eq!(
            Sync::FileProvider("Dropbox-Personal".into()).label(),
            "Dropbox"
        );
    }

    #[test]
    fn nosync_relocation_moves_the_store_and_links_it() {
        let dir = tempfile::tempdir().unwrap();
        let place = dir.path();
        std::fs::create_dir_all(place.join(".arbos/agents/root")).unwrap();
        std::fs::write(place.join(".arbos/notes.md"), "# notes").unwrap();
        let target = relocate(place, Relocation::Nosync).unwrap();
        assert_eq!(target, place.join(".arbos.nosync"));
        assert!(place.join(".arbos.nosync/notes.md").exists());
        assert!(
            std::fs::symlink_metadata(place.join(".arbos"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(place.join(".arbos/notes.md")).unwrap(),
            "# notes"
        );
        assert_eq!(relocated(place), Some(place.join(".arbos.nosync")));
        // Twice is refused.
        assert!(relocate(place, Relocation::Nosync).is_err());
    }

    #[test]
    fn nosync_relocation_excludes_the_new_name_from_the_projects_git() {
        let dir = tempfile::tempdir().unwrap();
        let place = dir.path();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(place)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("no git; skipping");
            return;
        }
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        relocate(place, Relocation::Nosync).unwrap();
        let exclude = std::fs::read_to_string(place.join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|l| l == ".arbos.nosync/"), "{exclude}");
        // Idempotent.
        crate::files::exclude_locally(place, &[".arbos.nosync/", ".arbos/"]);
        let again = std::fs::read_to_string(place.join(".git/info/exclude")).unwrap();
        assert_eq!(again.matches(".arbos.nosync/").count(), 1, "{again}");
        assert!(again.lines().any(|l| l == ".arbos/"), "{again}");
    }
}
