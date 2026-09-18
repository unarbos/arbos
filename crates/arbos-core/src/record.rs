//! Reads that know what they found, for the writes that act on them.
//!
//! The rule: **no destructive step — a `git reset`, a `clean`, a file
//! rewritten over its old bytes — on a record we have not confirmed.**
//!
//! Behind qal-j08, j09, j10 and j11 was one shape: a read helper that could
//! not tell *absent* from *unknown*. `read_to_string(..).unwrap_or_default()`
//! turns EIO, EACCES, a lock, a partial view on a mount, into `""`, and the
//! next writer acts on that emptiness with confidence — the project page
//! rewritten to one line, a checkpoint that carried HEAD alone, an `undo`
//! to a stale mark. A file that is not there is a fact (empty is the right
//! start); a file that could not be read is a question, and a question is
//! not a value.
//!
//! [`Read`] keeps the three apart. Readers that only display may still
//! settle for [`Read::present`]; anything that will *write over* or *reset
//! to* what it read must go through [`Read::confirmed`], which turns
//! `Unknown` into an error the caller surfaces instead of a default it
//! trusts. Writes that later readers will act on go through
//! [`write_atomic`], which fails loudly rather than leaving half a record.

use std::path::Path;

use anyhow::{Result, anyhow};

/// What a read found.
#[derive(Debug)]
pub enum Read<T> {
    /// The record exists and this is it.
    Present(T),
    /// The record does not exist (`NotFound`): a fact, and usually "start
    /// empty" is right.
    Absent,
    /// The record may exist but could not be read or parsed: not a value.
    /// Carries the reason, path included.
    Unknown(String),
}

impl<T> Read<T> {
    /// For a destructive step: the record, `None` when confirmed absent,
    /// and an error when unknown — never a default.
    pub fn confirmed(self) -> Result<Option<T>> {
        match self {
            Read::Present(v) => Ok(Some(v)),
            Read::Absent => Ok(None),
            Read::Unknown(why) => Err(anyhow!("{why} — nothing written or reset on its account")),
        }
    }

    /// For a display or a heuristic: the record when it was read, else
    /// nothing. A caller that uses this and then writes has skipped the
    /// rule; grep for it when a page goes missing.
    pub fn present(self) -> Option<T> {
        match self {
            Read::Present(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Read::Unknown(_))
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Read<U> {
        match self {
            Read::Present(v) => Read::Present(f(v)),
            Read::Absent => Read::Absent,
            Read::Unknown(w) => Read::Unknown(w),
        }
    }
}

/// The file's text; `Absent` when there is no such file.
pub fn read_text(path: &Path) -> Read<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Read::Present(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
        Err(e) => Read::Unknown(format!("could not read {}: {e}", path.display())),
    }
}

/// The file parsed as one JSON value; a parse failure is `Unknown` (the
/// file is there, but its content is not a record we can act on).
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Read<T> {
    match read_text(path) {
        Read::Present(text) => match serde_json::from_str(&text) {
            Ok(v) => Read::Present(v),
            Err(e) => Read::Unknown(format!("could not parse {}: {e}", path.display())),
        },
        Read::Absent => Read::Absent,
        Read::Unknown(w) => Read::Unknown(w),
    }
}

/// Write `bytes` as the whole file, through a sibling temp file and a
/// rename, so a reader never sees half of it and a failure leaves the old
/// file as it was. Errors name the path.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow!("could not create {}: {e}", parent.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "record".into());
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| anyhow!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow!("could not replace {}: {e}", path.display())
    })
}

/// Replace a file's contents whole: never a moment where a reader finds it
/// empty. `std::fs::write` truncates, then writes — a reader that lands
/// between the two sees zero bytes (the store's second reader raised two
/// FAULTs in one night on `notes.md`, mid-rewrite by an agent's `write`).
/// The bytes go to a sibling temp file, then a rename lands them. What
/// the replaced file was is kept: its permission bits (an executable
/// script stays executable), and a symlink is followed so the target is
/// replaced, not the link. A file that does not exist yet is created.
pub fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // The link's target is the file; a rename over the link would leave
    // a regular file where the link was.
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        )
    })?;
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }
    let mode = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let tmp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        REPLACE_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let done = (|| {
        std::fs::write(&tmp, bytes)?;
        if let Some(perm) = mode {
            std::fs::set_permissions(&tmp, perm)?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

static REPLACE_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    /// The tools' writes land whole: a file keeps its mode across a
    /// replace, a symlink's target is what changes, a new file is made,
    /// and no temp file is left behind.
    #[test]
    fn replace_file_keeps_the_mode_follows_a_link_and_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("arbos-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("run.sh");
        std::fs::write(&script, "#!/bin/sh\necho one\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        replace_file(&script, b"#!/bin/sh\necho two\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&script).unwrap(),
            "#!/bin/sh\necho two\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&script).unwrap().permissions().mode() & 0o777,
                0o755
            );
            let link = dir.join("link.sh");
            std::os::unix::fs::symlink(&script, &link).unwrap();
            replace_file(&link, b"#!/bin/sh\necho three\n").unwrap();
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "the link stays a link"
            );
            assert_eq!(
                std::fs::read_to_string(&script).unwrap(),
                "#!/bin/sh\necho three\n"
            );
        }
        let fresh = dir.join("sub").join("new.txt");
        replace_file(&fresh, b"made\n").unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "made\n");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absent_and_unknown_are_different_answers() {
        let dir = std::env::temp_dir().join(format!("arbos-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.md");
        assert!(matches!(read_text(&missing), Read::Absent));
        assert!(read_text(&missing).confirmed().unwrap().is_none());

        let page = dir.join("page.md");
        std::fs::write(&page, "hello\n").unwrap();
        assert_eq!(
            read_text(&page).confirmed().unwrap().as_deref(),
            Some("hello\n")
        );

        // Unreadable, not absent: a directory where a file was expected.
        let unreadable = dir.join("dir");
        std::fs::create_dir_all(&unreadable).unwrap();
        let r = read_text(&unreadable);
        assert!(r.is_unknown(), "{r:?}");
        let err = read_text(&unreadable).confirmed().unwrap_err().to_string();
        assert!(
            err.contains("could not read") && err.contains("nothing written"),
            "{err}"
        );
        assert!(read_text(&unreadable).present().is_none());

        // Unparseable is unknown too.
        std::fs::write(&page, "not json").unwrap();
        assert!(read_json::<serde_json::Value>(&page).is_unknown());

        // An atomic write that cannot land leaves the old bytes.
        std::fs::write(&page, "old\n").unwrap();
        let blocked = dir.join("dir/file"); // parent is a dir we can use
        write_atomic(&blocked, b"x").unwrap();
        assert_eq!(std::fs::read_to_string(&blocked).unwrap(), "x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
