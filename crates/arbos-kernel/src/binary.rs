//! Which `arbos-kernel` file to start another kernel from.
//!
//! `std::env::current_exe()` names the file this process was started
//! from. On Linux that is `/proc/self/exe`, and when the file has since
//! been replaced by unlink-and-write (an update that swaps the binary) or
//! moved, the link reads `… (deleted)` and `Command::new` on it fails with
//! ENOENT. On arboslife every spawn was refused that way for two days
//! (JB-6): the worker daemon had outlived its own binary, and the new
//! build sat on disk at the very path it was started from.
//!
//! The answer: the path the process was started with, resolved at start
//! and kept; then `arbos-kernel` on PATH; and a line saying which was
//! used, because a daemon running a build older than the file it starts
//! kernels from is worth knowing about.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, bail};

static STARTED_AS: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Called once at start, before anything replaces the binary: `argv[0]`
/// resolved against the working directory (when it names a path) or
/// PATH (when it is a bare name).
pub fn remember_start() {
    arbos_core::binary_identity::remember_start();
    let _ = STARTED_AS.get_or_init(|| {
        let argv0 = std::env::args_os().next().map(PathBuf::from)?;
        resolve_argv0(
            &argv0,
            &std::env::current_dir().ok()?,
            std::env::var_os("PATH"),
        )
    });
}

fn resolve_argv0(
    argv0: &Path,
    cwd: &Path,
    path_var: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if argv0.components().count() > 1 || argv0.is_absolute() {
        let p = if argv0.is_absolute() {
            argv0.to_path_buf()
        } else {
            cwd.join(argv0)
        };
        return Some(std::fs::canonicalize(&p).unwrap_or(p));
    }
    on_path(argv0, path_var)
}

fn on_path(name: &Path, path_var: Option<std::ffi::OsString>) -> Option<PathBuf> {
    std::env::split_paths(&path_var?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// How the binary was chosen, for the log and the spawn result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub path: PathBuf,
    /// None when it is this very file; otherwise why it is not, and what
    /// was used instead.
    pub note: Option<String>,
}

/// The file to start a kernel from: this process's own binary when it
/// still exists, else the path this process was started with, else
/// `arbos-kernel` on PATH.
pub fn kernel_binary() -> Result<Chosen> {
    choose(
        std::env::current_exe().ok(),
        STARTED_AS.get().cloned().flatten(),
        on_path(Path::new("arbos-kernel"), std::env::var_os("PATH")),
    )
}

fn choose(
    current_exe: Option<PathBuf>,
    started_as: Option<PathBuf>,
    on_path: Option<PathBuf>,
) -> Result<Chosen> {
    let usable = |p: &PathBuf| p.is_file() && !p.to_string_lossy().ends_with(" (deleted)");
    if let Some(me) = &current_exe
        && usable(me)
    {
        return Ok(Chosen {
            path: me.clone(),
            note: None,
        });
    }
    let gone = current_exe
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown)".into());
    if let Some(p) = started_as.filter(usable) {
        return Ok(Chosen {
            path: p.clone(),
            note: Some(format!(
                "this daemon's own binary was replaced or moved under it ({gone}); kernels start from the path it was started with, {} — restart the daemon to run that build yourself",
                p.display()
            )),
        });
    }
    if let Some(p) = on_path.filter(usable) {
        return Ok(Chosen {
            path: p.clone(),
            note: Some(format!(
                "this daemon's own binary was replaced or moved under it ({gone}); kernels start from arbos-kernel on PATH, {} — restart the daemon to run that build yourself",
                p.display()
            )),
        });
    }
    bail!(
        "this daemon's own binary was replaced or moved under it ({gone}), and no arbos-kernel was found at the path it was started with or on PATH; restart the daemon from the new binary"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_binary_first_then_start_path_then_path_with_a_note() {
        let dir = std::env::temp_dir().join(format!("arbos-binary-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let me = dir.join("me");
        let started = dir.join("started");
        let on_path = dir.join("bin/arbos-kernel");
        for p in [&me, &started, &on_path] {
            std::fs::write(p, b"#!/bin/sh\n").unwrap();
        }
        let c = choose(
            Some(me.clone()),
            Some(started.clone()),
            Some(on_path.clone()),
        )
        .unwrap();
        assert_eq!(c.path, me);
        assert!(c.note.is_none());

        // Replaced by unlink-and-write: /proc/self/exe reads "(deleted)".
        let deleted = PathBuf::from(format!("{} (deleted)", me.display()));
        let c = choose(
            Some(deleted.clone()),
            Some(started.clone()),
            Some(on_path.clone()),
        )
        .unwrap();
        assert_eq!(c.path, started);
        assert!(c.note.as_deref().unwrap().contains("started with"), "{c:?}");
        assert!(
            c.note.as_deref().unwrap().contains("restart the daemon"),
            "{c:?}"
        );

        // Moved: the file is simply gone; the start path too; PATH has one.
        std::fs::remove_file(&me).unwrap();
        std::fs::remove_file(&started).unwrap();
        let c = choose(
            Some(me.clone()),
            Some(started.clone()),
            Some(on_path.clone()),
        )
        .unwrap();
        assert_eq!(c.path, on_path);
        assert!(c.note.as_deref().unwrap().contains("on PATH"), "{c:?}");

        // Nothing anywhere: a refusal that says what to do.
        let err = choose(Some(me), Some(started), None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("restart the daemon from the new binary"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn argv0_resolves_a_path_or_a_name() {
        let dir = std::env::temp_dir().join(format!("arbos-argv0-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/arbos-kernel"), b"").unwrap();
        std::fs::write(dir.join("k"), b"").unwrap();
        let path_var = Some(std::env::join_paths([dir.join("bin")]).unwrap());
        assert_eq!(
            resolve_argv0(Path::new("./k"), &dir, path_var.clone()),
            Some(std::fs::canonicalize(dir.join("k")).unwrap())
        );
        assert_eq!(
            resolve_argv0(Path::new("arbos-kernel"), &dir, path_var),
            Some(dir.join("bin/arbos-kernel"))
        );
        assert_eq!(resolve_argv0(Path::new("nope"), &dir, None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
